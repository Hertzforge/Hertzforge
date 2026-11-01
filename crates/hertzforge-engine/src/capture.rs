//! Capture seam: the disk-streaming boundary the engine writes raw frames through.
//!
//! The engine does not know or care whether the sink is an HDF5/SWMR file, a socket, or
//! a test vector. It hands whole blocks of raw, interleaved f32 frames plus the
//! timebase metadata (epoch + starting frame index) so the writer can author the
//! wrap/epoch/gap-aware index the capture layer requires.
//!
//! # TODO (deferred, documented on purpose)
//!
//! The real writer lives in the `hertzforge-capture` crate: HDF5/SWMR, pre-allocated object
//! graph committed before SWMR start, raw-on-ingest (compress only at export),
//! Fletcher32, chunk size bounded by the at-risk window, MATLAB-column-major +
//! netCDF-dimension-scale layout, and a crash/recovery path. That needs the native HDF5
//! C library (evaluate `hdf5-metno`) and is NOT implemented here. This seam
//! exists so the engine, the scheduler, and every safety test can run today with a pure
//! in-memory [`VecCaptureSink`]; wiring `hertzforge-capture` behind this trait is a separate
//! capture task. Backpressure policy (mark-gap-and-continue, never block, never
//! silent-overwrite) is realized by the sink returning [`CaptureError::WouldBlock`],
//! which the scheduler converts into a marked gap rather than stalling the RT thread.

use hertzforge_channel_table::ChannelTable;

/// Metadata accompanying a written block so the capture layer can index it against the
/// device timebase (epoch boundaries and gaps must be representable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockMeta {
    /// Device epoch this block belongs to (bumps on a device reset).
    pub epoch: u32,
    /// Global monotonic frame index of the FIRST frame in this block.
    pub start_frame: u64,
    /// Number of frames in this block.
    pub n_frames: u64,
    /// True if a time-gap precedes this block (dropped samples), so the writer marks a
    /// gap in the record index rather than presenting the data as contiguous.
    pub gap_before: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    /// The sink is full / disk is behind. Per the backpressure contract the scheduler
    /// marks a gap and continues; it never blocks the RT thread on the writer.
    WouldBlock,
    /// A fatal writer error (disk full, file closed). The run should stop.
    Fatal(String),
}

/// The disk-streaming boundary. A block is `n_frames * frame_len` interleaved f32s.
/// Implementations must be non-blocking from the caller's perspective (return
/// `WouldBlock` rather than parking the RT thread).
pub trait CaptureSink: Send {
    /// Called once at `configure()` so the sink can author its layout from the channel
    /// table (channel count, names, units). Default: no-op.
    fn on_configure(&mut self, _table: &ChannelTable) -> Result<(), CaptureError> {
        Ok(())
    }

    /// Write one block of raw interleaved frames with its timebase metadata.
    fn write_block(&mut self, samples: &[f32], meta: BlockMeta) -> Result<(), CaptureError>;

    /// Flush and finalize (close the file / commit the index). Default: no-op.
    fn finalize(&mut self) -> Result<(), CaptureError> {
        Ok(())
    }
}

/// In-memory capture sink for tests: accumulates every block plus its metadata so a
/// test can assert exactly what flowed backend -> ring -> processing -> capture.
#[derive(Debug, Default)]
pub struct VecCaptureSink {
    pub samples: Vec<f32>,
    pub blocks: Vec<BlockMeta>,
    pub configured: bool,
    pub finalized: bool,
}

impl VecCaptureSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Total frames captured across all blocks.
    pub fn total_frames(&self) -> u64 {
        self.blocks.iter().map(|b| b.n_frames).sum()
    }
}

impl CaptureSink for VecCaptureSink {
    fn on_configure(&mut self, _table: &ChannelTable) -> Result<(), CaptureError> {
        self.configured = true;
        Ok(())
    }

    fn write_block(&mut self, samples: &[f32], meta: BlockMeta) -> Result<(), CaptureError> {
        self.samples.extend_from_slice(samples);
        self.blocks.push(meta);
        Ok(())
    }

    fn finalize(&mut self) -> Result<(), CaptureError> {
        self.finalized = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec_sink_accumulates_blocks_and_meta() {
        let mut sink = VecCaptureSink::new();
        sink.on_configure(&ChannelTable::new()).unwrap();
        sink.write_block(
            &[1.0, 2.0, 3.0, 4.0],
            BlockMeta {
                epoch: 0,
                start_frame: 0,
                n_frames: 2,
                gap_before: false,
            },
        )
        .unwrap();
        sink.write_block(
            &[5.0, 6.0],
            BlockMeta {
                epoch: 1,
                start_frame: 100,
                n_frames: 1,
                gap_before: true,
            },
        )
        .unwrap();
        sink.finalize().unwrap();

        assert!(sink.configured);
        assert!(sink.finalized);
        assert_eq!(sink.samples, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(sink.total_frames(), 3);
        assert_eq!(sink.blocks[1].epoch, 1);
        assert!(sink.blocks[1].gap_before);
    }
}
