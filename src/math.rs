use std::ops::{Add, Div, Mul, Sub};

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3(pub f32, pub f32, pub f32);

impl Vec3 {
    pub fn from_array([x, y, z]: [f32; 3]) -> Self {
        Self(x, y, z)
    }

    pub fn to_array(self) -> [f32; 3] {
        [self.0, self.1, self.2]
    }

    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    pub fn axis(self, axis: usize) -> f32 {
        [self.0, self.1, self.2][axis]
    }

    pub fn max_component(self) -> f32 {
        self.0.max(self.1).max(self.2)
    }

    pub fn longest_axis(self) -> usize {
        if self.0 >= self.1 && self.0 >= self.2 {
            0
        } else if self.1 >= self.2 {
            1
        } else {
            2
        }
    }

    pub fn dot(self, other: Self) -> f32 {
        self.0 * other.0 + self.1 * other.1 + self.2 * other.2
    }

    pub fn cross(self, other: Self) -> Self {
        Self(
            self.1 * other.2 - self.2 * other.1,
            self.2 * other.0 - self.0 * other.2,
            self.0 * other.1 - self.1 * other.0,
        )
    }

    pub fn normalized(self) -> Self {
        self / self.dot(self).sqrt()
    }

    pub fn mix(self, other: Self, amount: f32) -> Self {
        self + (other - self) * amount
    }

    pub fn reflect(self, normal: Self) -> Self {
        self - normal * (2.0 * self.dot(normal))
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self(self.0 + other.0, self.1 + other.1, self.2 + other.2)
    }
}

impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self(self.0 - other.0, self.1 - other.1, self.2 - other.2)
    }
}

impl Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, scalar: f32) -> Self {
        Self(self.0 * scalar, self.1 * scalar, self.2 * scalar)
    }
}

impl Div<f32> for Vec3 {
    type Output = Self;
    fn div(self, scalar: f32) -> Self {
        Self(self.0 / scalar, self.1 / scalar, self.2 / scalar)
    }
}

pub fn smooth(start: f32, end: f32, value: f32) -> f32 {
    let t = ((value - start) / (end - start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut bits = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    bits ^= bits >> 16;
    bits = bits.wrapping_mul(0x7feb_352d);
    bits ^= bits >> 15;
    bits = bits.wrapping_mul(0x846c_a68b);
    bits ^= bits >> 16;
    (bits >> 8) as f32 / 16_777_216.0
}

pub fn noise(x: f32, y: f32, seed: u32) -> f32 {
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let u = smooth(0.0, 1.0, x - ix as f32);
    let v = smooth(0.0, 1.0, y - iy as f32);
    let a = hash(ix, iy, seed);
    let b = hash(ix + 1, iy, seed);
    let c = hash(ix, iy + 1, seed);
    let d = hash(ix + 1, iy + 1, seed);
    let low = a + (b - a) * u;
    let high = c + (d - c) * u;
    low + (high - low) * v
}

pub fn fractal(mut x: f32, mut y: f32, seed: u32, octaves: u32) -> f32 {
    let mut sum = 0.0;
    let mut weight = 0.5;
    for octave in 0..octaves {
        sum += weight * noise(x, y, seed.wrapping_add(octave * 29));
        (x, y) = (1.6 * x + 1.2 * y + 7.2, -1.2 * x + 1.6 * y + 3.1);
        weight *= 0.5;
    }
    sum / (1.0 - weight * 2.0)
}
