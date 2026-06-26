//! `f32` math methods backed by `libm`, available in `no_std`.
//!
//! On `std`/`test` builds these names resolve to the inherent `f32` methods
//! (inherent methods win over trait methods), so importing this trait there is a
//! harmless no-op. On the bare-metal `no_std` target the inherent methods don't
//! exist, so calls bind to these `libm`-backed implementations.

#[allow(dead_code)]
pub(crate) trait F32Ext {
    fn abs(self) -> f32;
    fn max(self, other: f32) -> f32;
    fn min(self, other: f32) -> f32;
    fn floor(self) -> f32;
    fn ceil(self) -> f32;
    fn round(self) -> f32;
    fn trunc(self) -> f32;
    fn fract(self) -> f32;
    fn sqrt(self) -> f32;
    fn sin(self) -> f32;
    fn cos(self) -> f32;
    fn powf(self, n: f32) -> f32;
    fn powi(self, n: i32) -> f32;
    fn rem_euclid(self, rhs: f32) -> f32;
}

impl F32Ext for f32 {
    #[inline]
    fn abs(self) -> f32 {
        libm::fabsf(self)
    }
    #[inline]
    fn max(self, other: f32) -> f32 {
        libm::fmaxf(self, other)
    }
    #[inline]
    fn min(self, other: f32) -> f32 {
        libm::fminf(self, other)
    }
    #[inline]
    fn floor(self) -> f32 {
        libm::floorf(self)
    }
    #[inline]
    fn ceil(self) -> f32 {
        libm::ceilf(self)
    }
    #[inline]
    fn round(self) -> f32 {
        libm::roundf(self)
    }
    #[inline]
    fn trunc(self) -> f32 {
        libm::truncf(self)
    }
    #[inline]
    fn fract(self) -> f32 {
        self - libm::truncf(self)
    }
    #[inline]
    fn sqrt(self) -> f32 {
        libm::sqrtf(self)
    }
    #[inline]
    fn sin(self) -> f32 {
        libm::sinf(self)
    }
    #[inline]
    fn cos(self) -> f32 {
        libm::cosf(self)
    }
    #[inline]
    fn powf(self, n: f32) -> f32 {
        libm::powf(self, n)
    }
    #[inline]
    fn powi(self, n: i32) -> f32 {
        libm::powf(self, n as f32)
    }
    #[inline]
    fn rem_euclid(self, rhs: f32) -> f32 {
        let r = libm::fmodf(self, rhs);
        if r < 0.0 { r + libm::fabsf(rhs) } else { r }
    }
}
