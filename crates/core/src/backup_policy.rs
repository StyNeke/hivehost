#[derive(Debug)]
pub struct BackupPolicy {
    retention: u8,
}

impl Default for BackupPolicy {
    fn default() -> Self {
        Self { retention: 3 }
    }
}

impl BackupPolicy {
    pub fn new(retention: u8) -> Result<Self, &'static str> {
        if !(1..=5).contains(&retention) {
            return Err("backup retention must be between 1 and 5");
        }
        Ok(Self { retention })
    }
    pub fn retention(&self) -> u8 {
        self.retention
    }
    pub fn excess_after_success(&self, verified_backups: usize) -> usize {
        verified_backups.saturating_sub(self.retention.into())
    }
}
