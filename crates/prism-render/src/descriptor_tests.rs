use super::*;

#[test]
fn gpu_light_size_is_64() {
    assert_eq!(std::mem::size_of::<GpuLight>(), 64);
    assert_eq!(GPU_LIGHT_SIZE, 64);
}

#[test]
fn gpu_light_offsets() {
    assert_eq!(std::mem::offset_of!(GpuLight, position), 0);
    assert_eq!(std::mem::offset_of!(GpuLight, color), 16);
    assert_eq!(std::mem::offset_of!(GpuLight, direction), 32);
    assert_eq!(std::mem::offset_of!(GpuLight, params), 48);
}
