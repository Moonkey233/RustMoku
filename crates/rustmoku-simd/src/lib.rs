//! Small audited SIMD boundary. Engine/core continue to forbid unsafe code.
//! Arithmetic wraps like the scalar oracle; the evaluator validates tighter
//! accumulator bounds so its actual operations never overflow.
#![deny(unsafe_op_in_unsafe_fn)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backend {
    avx2: bool,
}
impl Backend {
    pub const SCALAR: Self = Self { avx2: false };
    pub fn detect() -> Self {
        Self::avx2().unwrap_or(Self::SCALAR)
    }
    pub fn avx2() -> Option<Self> {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        if std::is_x86_feature_detected!("avx2") {
            return Some(Self { avx2: true });
        }
        None
    }
    pub const fn name(self) -> &'static str {
        if self.avx2 { "avx2" } else { "scalar" }
    }
    pub fn delta32(self, sums: &mut [i32; 32], old: &[i8; 32], new: &[i8; 32]) {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        if self.avx2 {
            // SAFETY: private flag is only set after runtime AVX2 detection;
            // exact array lengths satisfy all unaligned loads/stores below.
            unsafe {
                x86::delta32(sums, old, new);
            }
            return;
        }
        for ((sum, &old), &new) in sums.iter_mut().zip(old).zip(new) {
            *sum = sum
                .wrapping_add(i32::from(new))
                .wrapping_sub(i32::from(old));
        }
    }
    pub fn dot160(self, input: &[i32; 160], weights: &[i8; 160]) -> i32 {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        if self.avx2 {
            // SAFETY: private flag proves runtime CPU support; fixed arrays
            // cover each 32-byte input load and eight-byte weight load.
            return unsafe { x86::dot160(input, weights) };
        }
        input.iter().zip(weights).fold(0i32, |sum, (&a, &b)| {
            sum.wrapping_add(a.wrapping_mul(i32::from(b)))
        })
    }
}
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86 {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;
    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn delta32(sums: &mut [i32; 32], old: &[i8; 32], new: &[i8; 32]) {
        for i in (0..32).step_by(8) {
            // SAFETY: i <= 24; sums has eight remaining i32 lanes (32 bytes),
            // old/new have eight bytes. loadl/loadu/storeu require no alignment.
            // Caller guarantees AVX2. Arrays cannot alias the exclusive sums.
            unsafe {
                let a = _mm256_loadu_si256(sums.as_ptr().add(i).cast());
                let b = _mm256_cvtepi8_epi32(_mm_loadl_epi64(old.as_ptr().add(i).cast()));
                let c = _mm256_cvtepi8_epi32(_mm_loadl_epi64(new.as_ptr().add(i).cast()));
                _mm256_storeu_si256(
                    sums.as_mut_ptr().add(i).cast(),
                    _mm256_add_epi32(a, _mm256_sub_epi32(c, b)),
                );
            }
        }
    }
    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn dot160(input: &[i32; 160], weights: &[i8; 160]) -> i32 {
        let mut sum = _mm256_setzero_si256();
        for i in (0..160).step_by(8) {
            // SAFETY: i <= 152, so input has 32 bytes and weights eight bytes
            // remaining. Both loads are unaligned. Caller guarantees AVX2.
            unsafe {
                let a = _mm256_loadu_si256(input.as_ptr().add(i).cast());
                let b = _mm256_cvtepi8_epi32(_mm_loadl_epi64(weights.as_ptr().add(i).cast()));
                sum = _mm256_add_epi32(sum, _mm256_mullo_epi32(a, b));
            }
        }
        let mut lanes = [0i32; 8];
        // SAFETY: lanes provides exactly 32 writable bytes; unaligned store,
        // AVX2 guaranteed by caller, and no overlapping references.
        unsafe {
            _mm256_storeu_si256(lanes.as_mut_ptr().cast(), sum);
        }
        lanes.into_iter().fold(0i32, i32::wrapping_add)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extreme_differential() {
        let Some(avx) = Backend::avx2() else {
            return;
        };
        for seed in [0, 1, 127, 255] {
            let old = std::array::from_fn(|i| (i * 31 + seed) as i8);
            let new = old.map(i8::wrapping_neg);
            let mut scalar = [i32::MAX; 32];
            let mut vector = scalar;
            Backend::SCALAR.delta32(&mut scalar, &old, &new);
            avx.delta32(&mut vector, &old, &new);
            assert_eq!(scalar, vector);
            let input = std::array::from_fn(|i| if i % 2 == 0 { i32::MIN } else { 255 });
            let weights = std::array::from_fn(|i| (i * 17 + seed) as i8);
            assert_eq!(
                Backend::SCALAR.dot160(&input, &weights),
                avx.dot160(&input, &weights)
            );
        }
    }
}
