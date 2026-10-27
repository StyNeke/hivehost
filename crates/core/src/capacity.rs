#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resources {
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub cpu_millis: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CapacityError {
    InvalidRequest,
    Memory,
    Disk,
    Cpu,
}

pub fn admit(
    requested: Resources,
    budget: Resources,
    reserved: Resources,
) -> Result<(), CapacityError> {
    if requested.memory_bytes == 0 || requested.disk_bytes == 0 || requested.cpu_millis == 0 {
        return Err(CapacityError::InvalidRequest);
    }
    if reserved
        .memory_bytes
        .checked_add(requested.memory_bytes)
        .is_none_or(|total| total > budget.memory_bytes)
    {
        return Err(CapacityError::Memory);
    }
    if reserved
        .disk_bytes
        .checked_add(requested.disk_bytes)
        .is_none_or(|total| total > budget.disk_bytes)
    {
        return Err(CapacityError::Disk);
    }
    if reserved
        .cpu_millis
        .checked_add(requested.cpu_millis)
        .is_none_or(|total| total > budget.cpu_millis)
    {
        return Err(CapacityError::Cpu);
    }
    Ok(())
}
