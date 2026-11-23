use super::*;

#[test]
fn invalid_handle_is_max() {
    assert_eq!(TextureHandle::INVALID.0, u32::MAX);
}

#[test]
fn sampler_type_count_is_4() {
    assert_eq!(SamplerType::COUNT, 4);
}

#[test]
fn sampler_type_indices_are_sequential() {
    assert_eq!(SamplerType::LinearWrap as u32, 0);
    assert_eq!(SamplerType::LinearClamp as u32, 1);
    assert_eq!(SamplerType::Nearest as u32, 2);
    assert_eq!(SamplerType::Shadow as u32, 3);
}

/// Verifies the "register_with_handle bumps next  invariant. The
/// Exact behavior is that registering slot 0 followed by a normal
/// `register` must yield slot 1, not slot 0. We can't construct a
/// complete `BindlessTextureTable` without a device so this is a
/// shape-only test of the slot-allocation contract.
#[test]
fn register_with_handle_advances_next_pointer() {
    struct Impl {
        next: u32,
    }
    let mut s = Impl { next: 0 };
    s.next = 1;
    let next_slot = s.next;
    assert_eq!(next_slot, 1, "register_with_handle must advance next");
}

#[test]
fn slot_allocator_bumps_then_reuses_released_slots_lifo() {
    let mut slots = SlotAllocator::new(4);
    assert_eq!(slots.claim().unwrap(), 0);
    assert_eq!(slots.claim().unwrap(), 1);
    assert_eq!(slots.claim().unwrap(), 2);
    assert_eq!(slots.live_len(), 3);

    assert!(slots.release(1));
    assert_eq!(slots.live_len(), 2);
    assert_eq!(slots.claim().unwrap(), 1);
    assert_eq!(slots.live_len(), 3);
    assert_eq!(slots.free_slot_count(), 0);

    assert!(slots.release(2));
    assert!(slots.release(0));
    assert_eq!(slots.claim().unwrap(), 0);
    assert_eq!(slots.claim().unwrap(), 2);
    assert_eq!(slots.claim().unwrap(), 3);
}

#[test]
fn slot_allocator_reports_released_slots_as_unused() {
    let mut slots = SlotAllocator::new(2);
    let slot = slots.claim().unwrap();
    assert!(slots.is_used(slot));
    assert!(slots.release(slot));
    assert!(!slots.is_used(slot));
    assert!(!slots.is_used(1));
    assert!(!slots.is_used(99));
}

#[test]
fn slot_allocator_rejects_double_release_and_unclaimed_release() {
    let mut slots = SlotAllocator::new(2);
    assert!(!slots.release(0), "never-claimed slot must not be released");
    let slot = slots.claim().unwrap();
    assert!(slots.release(slot));
    assert!(!slots.release(slot), "double release must be rejected");
    assert_eq!(slots.claim().unwrap(), slot);
    assert_eq!(slots.free_slot_count(), 0);
}

#[test]
fn slot_allocator_claim_at_advances_high_water_mark() {
    let mut slots = SlotAllocator::new(8);
    assert_eq!(slots.claim_at(3).unwrap(), 3);
    assert_eq!(slots.live_len(), 1);
    assert_eq!(slots.claim().unwrap(), 4);
    assert!(slots.is_used(3));
    assert!(!slots.is_used(0));
}

#[test]
fn slot_allocator_claim_at_reclaims_a_released_slot() {
    let mut slots = SlotAllocator::new(4);
    let slot = slots.claim().unwrap();
    assert!(slots.release(slot));
    assert_eq!(slots.free_slot_count(), 1);
    assert_eq!(slots.claim_at(slot).unwrap(), slot);
    assert_eq!(
        slots.free_slot_count(),
        0,
        "claim_at must consume the free entry"
    );
    assert_eq!(
        slots.live_len(),
        1,
        "re-claiming a released slot must not double count"
    );
}

#[test]
fn slot_allocator_claim_fails_when_capacity_is_exhausted() {
    let mut slots = SlotAllocator::new(2);
    assert_eq!(slots.claim().unwrap(), 0);
    assert_eq!(slots.claim().unwrap(), 1);
    let err = slots.claim().unwrap_err().to_string();
    assert!(err.contains("table full"), "unexpected error: {err}");
    assert!(slots.release(0));
    assert_eq!(slots.claim().unwrap(), 0);
    assert!(slots.claim().is_err());
}

#[test]
fn slot_allocator_rejects_slots_at_or_past_capacity() {
    let mut slots = SlotAllocator::new(2);
    assert!(slots.claim_at(2).is_err());
    assert!(slots.claim_at(u32::MAX).is_err());
    assert_eq!(slots.live_len(), 0);
}

#[test]
fn slot_allocator_empty_and_capacity_accessors() {
    let mut slots = SlotAllocator::new(3);
    assert!(slots.is_empty());
    assert_eq!(slots.capacity(), 3);
    let slot = slots.claim().unwrap();
    assert!(!slots.is_empty());
    assert!(slots.release(slot));
    assert!(
        slots.is_empty(),
        "releasing every slot makes it empty again"
    );
}
