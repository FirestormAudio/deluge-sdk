//! Minimal real-input power spectrum for the analysis task.
//!
//! A Hann window and an in-place iterative radix-2 complex FFT, with the
//! twiddle and window tables built at compile time. Written for clarity over
//! speed: a 512-point transform is ~2.3k butterflies, which the analysis task's
//! 50 ms cadence absorbs easily.

/// Transform length in real samples.
pub(crate) const N: usize = 512;

/// Number of non-redundant bins of a real-input transform (0 … N/2).
pub(crate) const BINS: usize = N / 2 + 1;

const LOG2_N: u32 = N.trailing_zeros();
const TWO_PI: f64 = 2.0 * core::f64::consts::PI;

const _: () = assert!(N.is_power_of_two());

/// `sin(x)` for `x` in `[-π/2, π/2]` (Taylor series, f64).
const fn sin_kernel(x: f64) -> f64 {
    let x2 = x * x;
    let mut term = x;
    let mut sum = x;
    let mut k = 1;
    while k < 10 {
        term = -term * x2 / ((2 * k) as f64 * (2 * k + 1) as f64);
        sum += term;
        k += 1;
    }
    sum
}

/// `sin(x)` for `x` in `[0, 2π)`.
const fn sin(x: f64) -> f64 {
    const PI: f64 = core::f64::consts::PI;
    const HALF_PI: f64 = PI / 2.0;
    let r = if x > PI { x - TWO_PI } else { x };
    if r > HALF_PI {
        sin_kernel(PI - r)
    } else if r < -HALF_PI {
        sin_kernel(-PI - r)
    } else {
        sin_kernel(r)
    }
}

/// `cos(x)` for `x` in `[0, 2π)`.
const fn cos(x: f64) -> f64 {
    let s = x + core::f64::consts::FRAC_PI_2;
    sin(if s >= TWO_PI { s - TWO_PI } else { s })
}

/// `(cos, -sin)` of `2πk/N` for `k` in `0 … N/2`: the forward-transform twiddles.
const TWIDDLES: [(f32, f32); N / 2] = {
    let mut t = [(0.0, 0.0); N / 2];
    let mut k = 0;
    while k < N / 2 {
        let a = TWO_PI * k as f64 / N as f64;
        t[k] = (cos(a) as f32, -sin(a) as f32);
        k += 1;
    }
    t
};

/// Periodic Hann window coefficients.
const HANN: [f32; N] = {
    let mut t = [0.0; N];
    let mut i = 0;
    while i < N {
        t[i] = (0.5 * (1.0 - cos(TWO_PI * i as f64 / N as f64))) as f32;
        i += 1;
    }
    t
};

/// Applies a Hann window to `buf` in place.
pub(crate) fn apply_hann(buf: &mut [f32; N]) {
    for (s, w) in buf.iter_mut().zip(HANN) {
        *s *= w;
    }
}

/// In-place forward FFT of the complex signal `(re, im)`.
fn fft(re: &mut [f32; N], im: &mut [f32; N]) {
    // Bit-reversal permutation.
    for i in 0..N {
        let j = i.reverse_bits() >> (usize::BITS - LOG2_N);
        if j > i {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    // Butterflies, doubling the span each stage.
    let mut len = 2;
    while len <= N {
        let half = len / 2;
        let stride = N / len;
        for start in (0..N).step_by(len) {
            for k in 0..half {
                let (wr, wi) = TWIDDLES[k * stride];
                let (a, b) = (start + k, start + k + half);
                let tr = re[b] * wr - im[b] * wi;
                let ti = re[b] * wi + im[b] * wr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len *= 2;
    }
}

/// Writes the squared magnitude of bins `0 … N/2` of the real signal `x` into `out`.
pub(crate) fn power_spectrum(x: &[f32; N], out: &mut [f32; BINS]) {
    let mut re = *x;
    let mut im = [0.0f32; N];
    fft(&mut re, &mut im);
    for (o, (r, i)) in out.iter_mut().zip(re.iter().zip(&im)) {
        *o = r * r + i * i;
    }
}
