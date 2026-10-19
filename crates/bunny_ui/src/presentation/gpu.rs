use super::Unavailable;
use super::backend::{PresentationSender, PresentationTicket};
use crate::gpu::walk::{FrameBatches, RunAtlas, RunKind};
use crate::image_engine::{ImageSource, PixelFormat};
use crate::layout::{DisplayList, DrawCommand};

/// A scalar-only attempt, allocated only for a live observer. Dropping without
/// a native commit explicitly reports the unqualified rendering path.
pub struct FrameObservation {
    sender: PresentationSender,
    generation: u64,
    finished: bool,
}

impl FrameObservation {
    /// Selected scalar slot, without retaining its image source.
    #[must_use]
    pub fn feed(&self) -> Option<crate::image_engine::FeedKey> {
        self.sender.feed()
    }

    #[must_use]
    pub fn begin(sender: &PresentationSender, display: &DisplayList) -> Option<Self> {
        let feed = sender.feed()?;
        let mut generation = None;
        for command in display.as_slice() {
            let DrawCommand::Image { source, .. } = command else {
                continue;
            };
            let mut inner = source;
            let mut transformed = false;
            while let ImageSource::Faded { inner: nested, .. }
            | ImageSource::Mirrored { inner: nested, .. } = inner
            {
                inner = nested;
                transformed = true;
            }
            if let ImageSource::Feed {
                key,
                generation: next,
                format,
                ..
            } = inner
                && *key == feed.raw()
            {
                if transformed
                    || *format != PixelFormat::Rgba8
                    || generation.is_some_and(|prior| prior != *next)
                {
                    sender.unavailable(Unavailable::UnsupportedSnapshot);
                    return None;
                }
                generation = Some(*next);
            }
        }
        let Some(generation) = generation else {
            sender.unavailable(Unavailable::NotDrawn);
            return None;
        };
        sender.submitted(generation).then(|| Self {
            sender: sender.clone(),
            generation,
            finished: false,
        })
    }

    pub fn unavailable(mut self, reason: Unavailable) {
        self.sender.unavailable(reason);
        self.finished = true;
    }

    /// Check the final live texture after all upload/retry work and a run that
    /// actually draws it. Display-list identity alone cannot prove either.
    #[must_use]
    pub fn commit(
        mut self,
        atlas: &RunAtlas,
        batches: &FrameBatches,
    ) -> Option<PresentationTicket> {
        self.finished = true;
        let key = self.sender.feed()?;
        let Some(entry) = atlas.live.get(&key.raw()) else {
            self.sender.unavailable(Unavailable::NotDrawn);
            return None;
        };
        if entry.stale || entry.generation != self.generation || entry.format != PixelFormat::Rgba8
        {
            self.sender.unavailable(Unavailable::UploadFailed);
            return None;
        }
        let drawn = batches.runs.iter().any(|run| {
            matches!(run.kind, RunKind::Live(index)
                if batches.textures.get(usize::from(index)) == Some(&entry.id))
                && run.count > 0
                && run.bounds.0 < run.bounds.2
                && run.bounds.1 < run.bounds.3
        });
        if !drawn {
            self.sender.unavailable(Unavailable::NotDrawn);
            return None;
        }
        self.sender.commit(self.generation)
    }
}

impl Drop for FrameObservation {
    fn drop(&mut self) {
        if !self.finished {
            self.sender.unavailable(Unavailable::NativeCompositing);
        }
    }
}
