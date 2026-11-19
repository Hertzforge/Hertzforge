/* hertzforge_module_abi.h - COMMITTED C-header floor for the Hertzforge module ABI (Layer 4).
 *
 * This header is the stable-layout contract for module cdylibs. CI diffs the layout
 * generated from `crates/hertzforge-module-abi/src/lib.rs` against this file and fails on any
 * non-additive change. It is the floor: even if the `#[repr(C)]` source is refactored,
 * a module compiled against this header must keep working. Keep the two in lockstep.
 *
 * ABI version: 0.3. 0.2 added the HERTZFORGE_LOOP_DELEGATED arm (additive). 0.3 removed the
 * module licensing surface (HertzforgeEntitlement, the manifest's feature codes, configure's
 * entitlement argument, HERTZFORGE_ABI_ENTITLEMENT_DENIED); no module was built against 0.2.
 * Status value 2 is retired and never reused.
 */
#ifndef HERTZFORGE_MODULE_ABI_H
#define HERTZFORGE_MODULE_ABI_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct HertzforgeAbiVersion {
    uint32_t major;
    uint32_t minor;
} HertzforgeAbiVersion;

#define HERTZFORGE_ABI_VERSION_MAJOR 0u
#define HERTZFORGE_ABI_VERSION_MINOR 3u

typedef enum HertzforgeAbiStatus {
    HERTZFORGE_ABI_OK = 0,
    HERTZFORGE_ABI_ERROR = 1,
    /* 2 retired in 0.3 (was HERTZFORGE_ABI_ENTITLEMENT_DENIED); never reuse it. */
    HERTZFORGE_ABI_DIM_MISMATCH = 3,
    HERTZFORGE_ABI_NOT_CONFIGURED = 4,
    HERTZFORGE_ABI_CLOCK_REFUSED = 5
} HertzforgeAbiStatus;

typedef enum HertzforgeLoopKind {
    HERTZFORGE_LOOP_OPEN = 0,
    HERTZFORGE_LOOP_CLOSED = 1,
    /* 0.2 additive: the module drives a smart controller's native mode (config +
     * start + sparse level commands) while the DEVICE generates every drive sample.
     * It never fills drive_ptr; the delegated vtable shape is NOT designed yet. */
    HERTZFORGE_LOOP_DELEGATED = 2
} HertzforgeLoopKind;

typedef struct HertzforgeTickCtx {
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
} HertzforgeTickCtx;

typedef struct HertzforgeModuleManifest {
    HertzforgeAbiVersion abi_version;
    const char* name;
    HertzforgeLoopKind loop_kind;
} HertzforgeModuleManifest;

typedef void* HertzforgeModuleHandle;

typedef struct HertzforgeModuleVTable {
    HertzforgeModuleHandle (*create)(void);
    void (*destroy)(HertzforgeModuleHandle);
    HertzforgeAbiStatus (*configure)(HertzforgeModuleHandle);
    HertzforgeAbiStatus (*arm)(HertzforgeModuleHandle);
    HertzforgeAbiStatus (*tick)(HertzforgeModuleHandle, HertzforgeTickCtx*);
    HertzforgeAbiStatus (*stop)(HertzforgeModuleHandle);
} HertzforgeModuleVTable;

typedef struct HertzforgeModuleDescriptor {
    HertzforgeModuleManifest manifest;
    HertzforgeModuleVTable vtable;
} HertzforgeModuleDescriptor;

/* Every module cdylib exports this symbol. */
const HertzforgeModuleDescriptor* hertzforge_module_entry(void);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* HERTZFORGE_MODULE_ABI_H */
