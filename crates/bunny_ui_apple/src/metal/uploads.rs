//! A staged generation is optimistic until its owning command completes.
//!
//! Keep scalar texture/generation stamps with the existing frame slot. A
//! failed command invalidates only the generation it attempted; later uploads
//! keep their cache entries. No callback, frame bytes or extra resource owner.

use super::{FrameSlot, MetalGround, RunAtlas, STATUS_COMPLETED, Sels, msg_u64};

#[derive(Clone, Copy)]
pub(super) struct LiveUpload {
    id: u64,
    generation: u64,
}

#[cfg(test)]
impl LiveUpload {
    pub(super) const fn for_test(id: u64, generation: u64) -> Self {
        Self { id, generation }
    }
}

impl FrameSlot {
    /// Called only after a non-null command was committed. Pending copies no
    /// longer belong to the next walk; the slot already retains their command.
    pub(super) fn submitted_uploads(&mut self, ground: &mut MetalGround, atlas: &RunAtlas) {
        #[cfg(test)]
        {
            self.failed_upload_for_test = false;
        }
        self.uploads.clear();
        self.uploads
            .extend(ground.pending.drain(..).filter_map(|copy| {
                atlas
                    .live
                    .values()
                    .find(|entry| entry.id == copy.id)
                    .map(|entry| LiveUpload {
                        id: entry.id,
                        generation: entry.generation,
                    })
            }));
    }

    /// In-flight outcomes leave the stamps untouched. Terminal outcomes are
    /// consumed exactly once, before the native command can be released.
    pub(super) fn settle_uploads(&mut self, status: u64, atlas: &mut RunAtlas) -> bool {
        if status < STATUS_COMPLETED {
            return false;
        }
        if status == STATUS_COMPLETED {
            self.uploads.clear();
            return false;
        }
        let mut invalidated = false;
        for upload in self.uploads.drain(..) {
            for entry in atlas.live.values_mut() {
                if entry.id == upload.id && entry.generation == upload.generation {
                    entry.stale = true;
                    invalidated = true;
                }
            }
        }
        invalidated
    }
}

/// Check failures before retained-frame/patch shortcuts can bypass a walk.
/// Frames without live uploads add no command-status queries here.
pub(super) fn poll_uploads(slots: &mut [FrameSlot; 3], atlas: &mut RunAtlas, sels: &Sels) -> bool {
    let mut invalidated = false;
    for slot in slots {
        if !slot.uploads.is_empty() {
            let status = unsafe { msg_u64(slot.command, sels.status) };
            #[cfg(test)]
            let status = if slot.failed_upload_for_test {
                5
            } else {
                status
            };
            invalidated |= slot.settle_uploads(status, atlas);
        }
    }
    invalidated
}
