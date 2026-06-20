//! External-oracle correctness: compare `deluge-fft` against `rustfft` over
//! randomized inputs and a sweep of sizes.
//!
//! The in-crate tests anchor correctness to a naive reference DFT at a couple of
//! sizes; this test adds an *independent*, widely-used implementation as the
//! oracle and fuzzes the input, so a systematic error in the radix-4/8 or
//! real-input paths that happened to agree with the in-crate DFT would still be
//! caught here.
//!
//! Host-only: `rustfft` is a large std crate and cross-compiling it under QEMU
//! for every size would dominate the test run. The portable-SIMD path exercised
//! here is the same code that runs on the device (only the NEON *lowering*
//! differs), and the NEON path is separately checked against the in-crate DFT
//! in the QEMU bucket.
#![cfg(not(target_arch = "arm"))]
#![feature(generic_const_exprs)]
#![allow(incomplete_features)]

use deluge_fft::{Complex, Fft, RealFft};
use rustfft::num_complex::Complex as C32;
use rustfft::FftPlanner;

/// Tiny deterministic PRNG (xorshift64*) → f32 in [-1, 1]. Avoids a `rand` dep
/// and keeps failures reproducible from the seed.
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn next_f32(&mut self) -> f32 {
        // top 24 bits → [0,1), then map to [-1,1)
        let u = (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32;
        u * 2.0 - 1.0
    }
}

/// f32 FFT error grows with size; this bound is loose enough to never flake yet
/// far tighter than the O(N) error a genuinely wrong transform would produce.
fn tol(n: usize) -> f32 {
    1e-2 * (n as f32).sqrt()
}

macro_rules! complex_oracle {
    ($n:literal) => {{
        const N: usize = $n;
        for seed in [1u64, 0xDEAD_BEEF, 0x1234_5678_9ABC_DEF0] {
            let mut rng = Rng(seed);
            let mut got = [Complex::ZERO; N];
            let mut want: Vec<C32<f32>> = Vec::with_capacity(N);
            for slot in got.iter_mut() {
                let (re, im) = (rng.next_f32(), rng.next_f32());
                slot.re = re;
                slot.im = im;
                want.push(C32::new(re, im));
            }

            Fft::<N, 4>::process_simd(&mut got);

            let mut planner = FftPlanner::new();
            planner.plan_fft_forward(N).process(&mut want);

            let mut max_err = 0f32;
            for (g, w) in got.iter().zip(want.iter()) {
                max_err = max_err.max((g.re - w.re).abs()).max((g.im - w.im).abs());
            }
            assert!(
                max_err < tol(N),
                "complex N={N} seed={seed:#x}: max_err={max_err} tol={}",
                tol(N)
            );
        }
    }};
}

macro_rules! real_oracle {
    ($n:literal) => {{
        const N: usize = $n;
        for seed in [2u64, 0xC0FF_EE00, 0x0BAD_F00D_DEAD_C0DE] {
            let mut rng = Rng(seed);
            let mut input = [0f32; N];
            let mut want: Vec<C32<f32>> = Vec::with_capacity(N);
            for slot in input.iter_mut() {
                let re = rng.next_f32();
                *slot = re;
                want.push(C32::new(re, 0.0));
            }

            let mut got = [Complex::ZERO; N / 2 + 1];
            RealFft::<N, 4>::process(&input, &mut got);

            let mut planner = FftPlanner::new();
            planner.plan_fft_forward(N).process(&mut want);

            let mut max_err = 0f32;
            for (i, g) in got.iter().enumerate() {
                max_err = max_err.max((g.re - want[i].re).abs()).max((g.im - want[i].im).abs());
            }
            assert!(
                max_err < tol(N),
                "real N={N} seed={seed:#x}: max_err={max_err} tol={}",
                tol(N)
            );
        }
    }};
}

#[test]
fn complex_fft_matches_rustfft() {
    complex_oracle!(8);
    complex_oracle!(16);
    complex_oracle!(32);
    complex_oracle!(64);
    complex_oracle!(128);
    complex_oracle!(256);
    complex_oracle!(512);
    complex_oracle!(1024);
}

#[test]
fn real_fft_matches_rustfft() {
    real_oracle!(16);
    real_oracle!(64);
    real_oracle!(256);
    real_oracle!(512);
    real_oracle!(1024);
}
