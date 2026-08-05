pub trait RandomSource: Send + Sync {
    fn fill(&self, destination: &mut [u8]);
}
