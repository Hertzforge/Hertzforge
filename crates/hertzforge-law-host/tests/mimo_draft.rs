//! The unstable `random-mimo` draft world elaborates: its WIT parses, its
//! `use`s resolve, and bindings generate for it. The host does not load
//! multiple-output laws yet; this only keeps the published draft valid.

mod draft {
    wasmtime::component::bindgen!({
        path: "../../wit/control-law",
        world: "random-mimo",
    });
}

use draft::hertzforge::control_law::mimo_types::{Complex, MatrixStack, MimoShape};

#[test]
fn the_mimo_draft_world_generates_bindings() {
    // A 2 x 2 stack on one line: row-major, both triangles of a Hermitian
    // matrix present.
    let m = MatrixStack {
        lines: 1,
        rows: 2,
        cols: 2,
        data: vec![
            Complex { re: 1.0, im: 0.0 },
            Complex { re: 0.5, im: 0.25 },
            Complex { re: 0.5, im: -0.25 },
            Complex { re: 2.0, im: 0.0 },
        ],
    };
    assert_eq!(m.data.len(), (m.lines * m.rows * m.cols) as usize);
    let shapes = [
        MimoShape::Square,
        MimoShape::MoreControlsThanDrives,
        MimoShape::FewerControlsThanDrives,
    ];
    assert_eq!(shapes.len(), 3);
}
