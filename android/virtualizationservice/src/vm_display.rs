// Copyright 2026 The Android Open Source Project
// SPDX-License-Identifier: Apache-2.0

//! VM/display ownership and stale-death protection, independent of Binder.

use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Error {
    AccessDenied,
    AlreadyAllocated,
    AlreadyRunning,
    NotReady,
    GenerationExhausted,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AccessDenied => "VM is not owned by the caller",
            Self::AlreadyAllocated => "VM CID is already allocated",
            Self::AlreadyRunning => "VM is already registered or its exit is unconfirmed",
            Self::NotReady => "VM display is not ready",
            Self::GenerationExhausted => "VM display generation exhausted",
        })
    }
}

impl std::error::Error for Error {}

enum VmSlot<V> {
    Allocated,
    Running(V),
    Retired,
}

struct Entry<V, D> {
    owner_uid: u32,
    generation: i64,
    vm: VmSlot<V>,
    display: Option<(i64, D)>,
}

/// Owns the same handles that keep VM and display death recipients linked.
/// The UID is captured at CID allocation, never supplied by an app argument.
pub(crate) struct VmDisplayRegistry<V, D> {
    entries: HashMap<u32, Entry<V, D>>,
    generation: i64,
}

impl<V, D> Default for VmDisplayRegistry<V, D> {
    fn default() -> Self {
        Self { entries: HashMap::new(), generation: 0 }
    }
}

impl<V, D> VmDisplayRegistry<V, D> {
    fn next_generation(&mut self) -> Result<i64, Error> {
        self.generation = self.generation.checked_add(1).ok_or(Error::GenerationExhausted)?;
        Ok(self.generation)
    }

    fn owned(&self, cid: u32, uid: u32) -> Result<&Entry<V, D>, Error> {
        self.entries.get(&cid).filter(|entry| entry.owner_uid == uid).ok_or(Error::AccessDenied)
    }

    fn owned_mut(&mut self, cid: u32, uid: u32) -> Result<&mut Entry<V, D>, Error> {
        self.entries.get_mut(&cid).filter(|entry| entry.owner_uid == uid).ok_or(Error::AccessDenied)
    }

    pub(crate) fn contains_cid(&self, cid: u32) -> bool {
        self.entries.contains_key(&cid)
    }

    pub(crate) fn running_vms(&self) -> impl Iterator<Item = (&u32, &V)> {
        self.entries.iter().filter_map(|(cid, entry)| match &entry.vm {
            VmSlot::Running(vm) => Some((cid, vm)),
            _ => None,
        })
    }

    pub(crate) fn allocate(&mut self, cid: u32, uid: u32) -> Result<(), Error> {
        if self.contains_cid(cid) {
            return Err(Error::AlreadyAllocated);
        }
        let generation = self.next_generation()?;
        self.entries.insert(
            cid,
            Entry { owner_uid: uid, generation, vm: VmSlot::Allocated, display: None },
        );
        Ok(())
    }

    /// Rebuild a live VM's registration after the global service restarts.
    /// Existing allocations must still match the kernel caller's full UID.
    pub(crate) fn adopt(&mut self, cid: u32, uid: u32) -> Result<(), Error> {
        if let Some(entry) = self.entries.get(&cid) {
            if entry.owner_uid != uid {
                Err(Error::AccessDenied)
            } else if matches!(&entry.vm, VmSlot::Retired) {
                Err(Error::AlreadyRunning)
            } else {
                Ok(())
            }
        } else {
            self.allocate(cid, uid)
        }
    }

    pub(crate) fn prepare_vm(&self, cid: u32, uid: u32) -> Result<i64, Error> {
        let entry = self.owned(cid, uid)?;
        if !matches!(&entry.vm, VmSlot::Allocated) {
            return Err(Error::AlreadyRunning);
        }
        Ok(entry.generation)
    }

    pub(crate) fn register_vm(&mut self, cid: u32, uid: u32, vm: V) -> Result<(), Error> {
        let entry = self.owned_mut(cid, uid)?;
        if !matches!(&entry.vm, VmSlot::Allocated) {
            return Err(Error::AlreadyRunning);
        }
        entry.vm = VmSlot::Running(vm);
        Ok(())
    }

    pub(crate) fn unregister_vm(&mut self, cid: u32, uid: u32) -> Result<(), Error> {
        self.owned(cid, uid)?;
        self.entries.remove(&cid);
        Ok(())
    }

    pub(crate) fn generation_matches(&self, cid: u32, generation: i64) -> bool {
        self.entries.get(&cid).is_some_and(|entry| entry.generation == generation)
    }

    /// Binder death does not prove that crosvm exited. Drop handles, but keep the CID
    /// unavailable until the owner confirms exit through unregister_vm or the service restarts.
    pub(crate) fn retire_vm(&mut self, cid: u32, generation: i64) {
        if let Some(entry) =
            self.entries.get_mut(&cid).filter(|entry| entry.generation == generation)
        {
            entry.vm = VmSlot::Retired;
            entry.display = None;
        }
    }

    pub(crate) fn reserve_display(&mut self, cid: u32, uid: u32) -> Result<i64, Error> {
        if !matches!(&self.owned(cid, uid)?.vm, VmSlot::Running(_)) {
            return Err(Error::NotReady);
        }
        self.next_generation()
    }

    pub(crate) fn set_display(
        &mut self,
        cid: u32,
        uid: u32,
        generation: i64,
        display: D,
    ) -> Result<Option<D>, Error> {
        let entry = self.owned_mut(cid, uid)?;
        if !matches!(&entry.vm, VmSlot::Running(_)) {
            return Err(Error::NotReady);
        }
        Ok(entry.display.replace((generation, display)).map(|(_, display)| display))
    }

    pub(crate) fn display(&self, cid: u32, uid: u32) -> Result<&D, Error> {
        let entry = self.owned(cid, uid)?;
        if !matches!(&entry.vm, VmSlot::Running(_)) {
            return Err(Error::NotReady);
        }
        entry.display.as_ref().map(|(_, display)| display).ok_or(Error::NotReady)
    }

    pub(crate) fn clear_display(
        &mut self,
        cid: u32,
        uid: u32,
        generation: i64,
    ) -> Result<Option<D>, Error> {
        self.owned(cid, uid)?;
        Ok(self.retire_display(cid, generation))
    }

    /// A late death notification must not clear a replacement display or reused CID.
    pub(crate) fn retire_display(&mut self, cid: u32, generation: i64) -> Option<D> {
        let entry = self.entries.get_mut(&cid)?;
        if entry.display.as_ref().is_some_and(|(current, _)| *current == generation) {
            entry.display.take().map(|(_, display)| display)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, VmDisplayRegistry};

    const OWNER: u32 = 10387;
    const OTHER: u32 = 10388;
    const WORK_PROFILE_OWNER: u32 = OWNER + 100000;
    const CID: u32 = 2048;

    fn running() -> VmDisplayRegistry<&'static str, &'static str> {
        let mut registry = VmDisplayRegistry::default();
        registry.allocate(CID, OWNER).unwrap();
        registry.register_vm(CID, OWNER, "vm-a").unwrap();
        registry
    }

    fn publish(
        registry: &mut VmDisplayRegistry<&str, &str>,
        cid: u32,
        uid: u32,
        name: &'static str,
    ) -> i64 {
        let generation = registry.reserve_display(cid, uid).unwrap();
        registry.set_display(cid, uid, generation, name).unwrap();
        generation
    }

    #[test]
    fn simultaneous_vms_are_selected_by_cid_and_full_android_uid() {
        let mut registry = running();
        registry.allocate(CID + 1, OTHER).unwrap();
        registry.register_vm(CID + 1, OTHER, "vm-b").unwrap();
        publish(&mut registry, CID, OWNER, "display-a");
        publish(&mut registry, CID + 1, OTHER, "display-b");
        assert_eq!(registry.display(CID, OWNER), Ok(&"display-a"));
        assert_eq!(registry.display(CID + 1, OTHER), Ok(&"display-b"));
        for uid in [OTHER, WORK_PROFILE_OWNER, 0] {
            assert_eq!(registry.display(CID, uid), Err(Error::AccessDenied));
        }
        assert_eq!(registry.display(CID + 1, OWNER), Err(Error::AccessDenied));
        assert_eq!(registry.display(CID + 2, OWNER), Err(Error::AccessDenied));
        assert_eq!(registry.running_vms().count(), 2);
    }

    #[test]
    fn only_allocator_can_register_and_live_allocations_cannot_be_replaced() {
        let mut registry = VmDisplayRegistry::<&str, &str>::default();
        registry.allocate(CID, OWNER).unwrap();
        assert!(registry.contains_cid(CID));
        assert_eq!(registry.allocate(CID, OTHER), Err(Error::AlreadyAllocated));
        assert_eq!(registry.prepare_vm(CID, OTHER), Err(Error::AccessDenied));
        assert_eq!(registry.register_vm(CID, OTHER, "foreign"), Err(Error::AccessDenied));
        assert_eq!(registry.reserve_display(CID, OWNER), Err(Error::NotReady));
        assert_eq!(registry.set_display(CID, OWNER, 1, "premature"), Err(Error::NotReady));
        registry.register_vm(CID, OWNER, "original").unwrap();
        assert_eq!(registry.prepare_vm(CID, OWNER), Err(Error::AlreadyRunning));
        assert_eq!(registry.register_vm(CID, OWNER, "replacement"), Err(Error::AlreadyRunning));
        assert_eq!(registry.running_vms().collect::<Vec<_>>(), vec![(&CID, &"original")]);
    }

    #[test]
    fn foreign_uid_cannot_publish_clear_or_unregister_an_owned_vm() {
        let mut registry = running();
        let display = publish(&mut registry, CID, OWNER, "original");
        assert_eq!(registry.reserve_display(CID, OTHER), Err(Error::AccessDenied));
        assert_eq!(registry.set_display(CID, OTHER, display, "foreign"), Err(Error::AccessDenied));
        assert_eq!(registry.clear_display(CID, OTHER, display), Err(Error::AccessDenied));
        assert_eq!(registry.unregister_vm(CID, OTHER), Err(Error::AccessDenied));
        assert_eq!(registry.display(CID, OWNER), Ok(&"original"));
    }

    #[test]
    fn failed_creation_can_cancel_its_unregistered_allocation() {
        let mut registry = running();
        registry.allocate(CID + 1, OWNER).unwrap();
        registry.unregister_vm(CID + 1, OWNER).unwrap();
        assert!(!registry.contains_cid(CID + 1));
        registry.allocate(CID + 1, OTHER).unwrap();
        assert_eq!(registry.prepare_vm(CID + 1, OWNER), Err(Error::AccessDenied));
        assert_eq!(registry.running_vms().collect::<Vec<_>>(), vec![(&CID, &"vm-a")]);
    }

    #[test]
    fn restarted_service_can_adopt_live_vms_but_not_overwrite_another_owner() {
        let mut registry = VmDisplayRegistry::<&str, &str>::default();
        registry.allocate(CID, OWNER).unwrap();
        let generation = registry.prepare_vm(CID, OWNER).unwrap();
        assert!(generation > 0);
        registry.adopt(CID, OWNER).unwrap();
        assert_eq!(registry.prepare_vm(CID, OWNER), Ok(generation));
        registry.register_vm(CID, OWNER, "surviving-vm").unwrap();
        registry.adopt(CID, OWNER).unwrap();
        assert_eq!(registry.adopt(CID, OTHER), Err(Error::AccessDenied));
        assert_eq!(registry.register_vm(CID, OTHER, "foreign-vm"), Err(Error::AccessDenied));
        publish(&mut registry, CID, OWNER, "surviving-display");
        assert_eq!(registry.display(CID, OWNER), Ok(&"surviving-display"));
        assert_eq!(registry.running_vms().collect::<Vec<_>>(), vec![(&CID, &"surviving-vm")]);
    }

    #[test]
    fn stale_display_death_and_clear_leave_replacement_attached() {
        let mut registry = running();
        let old = publish(&mut registry, CID, OWNER, "old");
        let current = publish(&mut registry, CID, OWNER, "current");
        assert_eq!(registry.retire_display(CID, old), None);
        assert_eq!(registry.clear_display(CID, OWNER, old), Ok(None));
        assert_eq!(registry.display(CID, OWNER), Ok(&"current"));
        assert_eq!(registry.clear_display(CID, OWNER, current), Ok(Some("current")));
        assert_eq!(registry.display(CID, OWNER), Err(Error::NotReady));
    }

    #[test]
    fn cid_reuse_revokes_old_handles_and_ignores_old_death_notifications() {
        let mut registry = VmDisplayRegistry::<&str, &str>::default();
        registry.allocate(CID, OWNER).unwrap();
        let old_vm = registry.prepare_vm(CID, OWNER).unwrap();
        registry.register_vm(CID, OWNER, "old-vm").unwrap();
        let old_display = publish(&mut registry, CID, OWNER, "old-display");
        registry.unregister_vm(CID, OWNER).unwrap();
        assert_eq!(registry.display(CID, OWNER), Err(Error::AccessDenied));
        registry.adopt(CID, OTHER).unwrap();
        let new_vm = registry.prepare_vm(CID, OTHER).unwrap();
        assert!(new_vm > old_vm);
        assert!(!registry.generation_matches(CID, old_vm));
        assert!(registry.generation_matches(CID, new_vm));
        registry.register_vm(CID, OTHER, "new-vm").unwrap();
        publish(&mut registry, CID, OTHER, "new-display");
        registry.retire_vm(CID, old_vm);
        assert_eq!(registry.retire_display(CID, old_display), None);
        assert_eq!(registry.display(CID, OTHER), Ok(&"new-display"));
    }

    #[test]
    fn binder_death_reserves_cid_until_crosvm_exit_is_confirmed() {
        let mut registry = VmDisplayRegistry::<&str, &str>::default();
        registry.allocate(CID, OWNER).unwrap();
        let generation = registry.prepare_vm(CID, OWNER).unwrap();
        registry.register_vm(CID, OWNER, "lost-manager").unwrap();
        let display_generation = publish(&mut registry, CID, OWNER, "display");
        registry.retire_vm(CID, generation);
        assert!(registry.contains_cid(CID));
        assert_eq!(registry.allocate(CID, OTHER), Err(Error::AlreadyAllocated));
        assert_eq!(registry.adopt(CID, OTHER), Err(Error::AccessDenied));
        assert_eq!(registry.adopt(CID, OWNER), Err(Error::AlreadyRunning));
        assert_eq!(registry.prepare_vm(CID, OWNER), Err(Error::AlreadyRunning));
        assert_eq!(registry.register_vm(CID, OWNER, "replacement"), Err(Error::AlreadyRunning));
        assert_eq!(registry.reserve_display(CID, OWNER), Err(Error::NotReady));
        assert_eq!(
            registry.set_display(CID, OWNER, display_generation, "replacement"),
            Err(Error::NotReady)
        );
        assert_eq!(registry.display(CID, OWNER), Err(Error::NotReady));
        assert_eq!(registry.running_vms().count(), 0);
        registry.unregister_vm(CID, OWNER).unwrap();
        registry.allocate(CID, OTHER).unwrap();
    }

    #[test]
    fn generation_exhaustion_refuses_without_reusing_a_token() {
        let mut registry = running();
        publish(&mut registry, CID, OWNER, "existing");
        registry.generation = i64::MAX;
        assert_eq!(registry.reserve_display(CID, OWNER), Err(Error::GenerationExhausted));
        assert_eq!(registry.allocate(CID + 1, OTHER), Err(Error::GenerationExhausted));
        assert!(!registry.contains_cid(CID + 1));
        assert_eq!(registry.display(CID, OWNER), Ok(&"existing"));
    }
}
