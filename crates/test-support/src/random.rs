use folioharbor_application::ports::RandomSource;

#[derive(Clone, Copy, Debug)]
pub struct FixedRandom(u8);

impl FixedRandom {
    #[must_use]
    pub const fn new(byte: u8) -> Self {
        Self(byte)
    }
}

impl RandomSource for FixedRandom {
    fn fill(&self, destination: &mut [u8]) {
        destination.fill(self.0);
    }
}
