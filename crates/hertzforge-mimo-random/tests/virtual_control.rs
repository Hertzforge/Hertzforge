//! A8: physical and virtual control converge on the same plant.
//!
//! The rigid plate (12 accelerometer channels, 6 drives) is controlled twice to the
//! same physical motion: once on its 12 physical channels, once on its six
//! rigid-body degrees of freedom through a response transformation and a drive
//! transformation built here from the plant's geometry. The response
//! transformation is the pseudo-inverse of the sensors' rigid-body kinematics
//! (each row `[d, r x d]` of a sensor at `r` reading along `d`); the drive
//! transformation is the inverse of the drives' generalized forces (each column
//! `[d, r x d]` of a drive at `r` pushing along `d`). (The operator's version of
//! this builder belongs to the reference work package.)
mod common;

use common::*;
use hertzforge_hal_sim::plants::{NamedPlant, Placement, StandardPlant};
use hertzforge_mimo_random::{ControlSpace, MimoState};
use hertzforge_sdm::linalg::{self, Regularization};
use hertzforge_sdm::{MatrixStack, C64};

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `[d, (r - r0) x d]` for one placement.
fn rigid_row(p: &Placement, r0: [f64; 3]) -> [f64; 6] {
    let r = [
        p.position_m[0] - r0[0],
        p.position_m[1] - r0[1],
        p.position_m[2] - r0[2],
    ];
    let m = cross(r, p.direction);
    [
        p.direction[0],
        p.direction[1],
        p.direction[2],
        m[0],
        m[1],
        m[2],
    ]
}

/// `(T_r, T_d)` from the plant's geometry.
fn transforms(plant: &NamedPlant) -> (MatrixStack, MatrixStack) {
    let g = &plant.geometry;
    let r0 = g.reference_point_m;
    let n_in = g.inputs.len();
    let mut k = MatrixStack::zeros(1, n_in, 6);
    for (i, p) in g.inputs.iter().enumerate() {
        for (j, v) in rigid_row(p, r0).iter().enumerate() {
            k.set(0, i, j, C64::new(*v, 0.0));
        }
    }
    let n_d = g.drives.len();
    let mut b = MatrixStack::zeros(1, 6, n_d);
    for (j, p) in g.drives.iter().enumerate() {
        for (i, v) in rigid_row(p, r0).iter().enumerate() {
            b.set(0, i, j, C64::new(*v, 0.0));
        }
    }
    let (t_r, _) = linalg::pinv(&k, Regularization::None).unwrap();
    let (t_d, _) = linalg::pinv(&b, Regularization::None).unwrap();
    (t_r, t_d)
}

fn grid() -> Grid {
    Grid {
        fs_hz: 4096.0,
        nfft: 2048,
        band_hz: (26.0, 150.0),
    }
}

#[test]
fn a8_physical_and_virtual_control_converge_on_the_rigid_plate() {
    let grid = grid();
    let p = StandardPlant::RigidPlate12x6.build();
    let (t_r, t_d) = transforms(&p);
    // The transformations are what the plant's rigid-body claim says: T_r H T_d
    // is close to diagonal over the band.
    let h = exact_frf(&p, &grid);
    let ht = linalg::frf_transform(&t_r, &h, Some(&t_d)).unwrap();
    let mid = (grid.band().0 + grid.band().1) / 2;
    for i in 0..6 {
        for j in 0..6 {
            if i != j {
                let r = ht.get(mid, i, j).norm() / ht.get(mid, i, i).norm();
                assert!(r < 0.1, "cross term ({i},{j}) {r}");
            }
        }
    }

    // Physical: 12 controls, the reference reachable.
    let (cfg, table) = physical_config(&p, &grid, 0.5);
    let phys_ref = cfg.reference.sdm.clone();
    let mut phys = service(cfg, table, &p, &grid, "a8-physical");
    let cp = converge(&mut phys, 1.0, 20);

    // Virtual: the six rigid-body DOFs of the same physical reference, through
    // both transformations.
    let (mut cfg, table) = physical_config(&p, &grid, 0.5);
    let v_ref = linalg::congruence(&t_r, &phys_ref).unwrap();
    cfg.reference = reference(v_ref, &grid);
    cfg.control_space = ControlSpace::Virtual { response: t_r };
    cfg.drive_transform = Some(t_d);
    let mut virt = service(cfg, table, &p, &grid, "a8-virtual");
    let cv = converge(&mut virt, 1.0, 20);

    eprintln!("A8 physical (12 x 6): {cp:?}");
    eprintln!("A8 virtual (6 DOFs, 6 virtual drives): {cv:?}");
    for (name, c) in [("physical", &cp), ("virtual", &cv)] {
        assert!(
            c.loops_to_converge.is_some_and(|n| n <= 20),
            "{name} control did not converge: {c:?}"
        );
        assert_ne!(c.state, MimoState::Aborted, "{name}");
    }
}
