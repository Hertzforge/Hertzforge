/* eigen_module_abi.h - COMMITTED C-header floor for the Eigen module ABI (Layer 4).
 *
 * This header is the stable-layout contract for module cdylibs. CI diffs the layout
 * generated from `crates/eigen-module-abi/src/lib.rs` against this file and fails on any
 * non-additive change. It is the floor: even if the `#[repr(C)]` source is refactored,
 * a module compiled against this header must keep working. Keep the two in lockstep.
 *
 * ABI version: 0.3. 0.2 added the EIGEN_LOOP_DELEGATED arm (additive). 0.3 removed the
 * module licensing surface (EigenEntitlement, the manifest's feature codes, configure's
 * entitlement argument, EIGEN_ABI_ENTITLEMENT_DENIED); no module was built against 0.2.
 * Status value 2 is retired and never reused.
 */
#ifndef EIGEN_MODULE_ABI_H
#define EIGEN_MODULE_ABI_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct EigenAbiVersion {
    uint32_t major;
    uint32_t minor;
} EigenAbiVersion;

#define EIGEN_ABI_VERSION_MAJOR 0u
#define EIGEN_ABI_VERSION_MINOR 3u

typedef enum EigenAbiStatus {
    EIGEN_ABI_OK = 0,
    EIGEN_ABI_ERROR = 1,
    /* 2 retired in 0.3 (was EIGEN_ABI_ENTITLEMENT_DENIED); never reuse it. */
    EIGEN_ABI_DIM_MISMATCH = 3,
    EIGEN_ABI_NOT_CONFIGURED = 4,
    EIGEN_ABI_CLOCK_REFUSED = 5
} EigenAbiStatus;

typedef enum EigenLoopKind {
    EIGEN_LOOP_OPEN = 0,
    EIGEN_LOOP_CLOSED = 1,
    /* 0.2 additive: the module drives a smart controller's native mode (config +
     * start + sparse level commands) while the DEVICE generates every drive sample.
     * It never fills drive_ptr; the delegated vtable shape is NOT designed yet. */
    EIGEN_LOOP_DELEGATED = 2
} EigenLoopKind;

typedef struct EigenTickCtx {
    uint32_t n_control;
    uint32_t n_monitor;
    uint32_t n_drive;
    uint32_t n_reference;
    uint32_t n_frames;
    double   sample_rate_hz;
    const float* input_ptr;   /* interleaved, (n_control+n_monitor+n_reference)*n_frames */
    size_t   input_len;
    float*   drive_ptr;       /* module fills, n_drive*n_frames */
    size_t   drive_len;
    const void* reference_spec; /* opaque MIMO reference spec, or NULL */
} EigenTickCtx;

typedef struct EigenModuleManifest {
    EigenAbiVersion abi_version;
    const char* name;
    EigenLoopKind loop_kind;
} EigenModuleManifest;

typedef void* EigenModuleHandle;

typedef struct EigenModuleVTable {
    EigenModuleHandle (*create)(void);
    void (*destroy)(EigenModuleHandle);
    EigenAbiStatus (*configure)(EigenModuleHandle);
    EigenAbiStatus (*arm)(EigenModuleHandle);
    EigenAbiStatus (*tick)(EigenModuleHandle, EigenTickCtx*);
    EigenAbiStatus (*stop)(EigenModuleHandle);
} EigenModuleVTable;

typedef struct EigenModuleDescriptor {
    EigenModuleManifest manifest;
    EigenModuleVTable vtable;
} EigenModuleDescriptor;

/* Every module cdylib exports this symbol. */
const EigenModuleDescriptor* eigen_module_entry(void);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* EIGEN_MODULE_ABI_H */
