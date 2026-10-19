//! One selected watch per connection; read storage without rendering every node.
//!
//! ## Wiring
//! Requests fence selection changes. A retained ReadBinding is a weak lifetime
//! token, not an application callback. Polling it notices changes even when the
//! formatted text stays identical. Publication then samples again after layout,
//! and sends the observation before the existing view-metadata/frame pair.

use super::*;
use provenance::{ReadBinding, observation};
use wire::{StateRead, StateRefusal, StateReply, StateRequestId, StateTarget};

pub(super) struct StateWatch {
    request: StateRequestId,
    target: StateTarget,
    scene: Rc<str>,
    reading: Option<ReadBinding>,
}
impl Host {
    fn state_scene(&self, target: &StateTarget) -> Result<usize, StateRefusal> {
        if !self.observe_views {
            return Err(StateRefusal::Disabled);
        }
        if target.preview.len() > wire::STATE_TEXT_LIMIT {
            return Err(StateRefusal::Limit);
        }
        if target.generation != self.generation {
            return Err(StateRefusal::Generation);
        }
        let mut matches = self.scenes.iter().enumerate().filter(|(_, scene)| {
            scene.instance() == target.instance.as_ref()
                && scene.ident.as_ref() == target.preview
                && scene.variant == target.variant
        });
        let (index, scene) = matches.next().ok_or(StateRefusal::SceneMissing)?;
        if matches.next().is_some() {
            return Err(StateRefusal::AmbiguousScene);
        }
        if scene.poisoned.is_some() {
            return Err(StateRefusal::Poisoned);
        }
        Ok(index)
    }
    pub(super) fn observe_state(&mut self, request: StateRequestId, target: Option<StateTarget>) {
        if self
            .state_request
            .is_some_and(|previous| request <= previous)
        {
            self.state_refused(request, StateRefusal::StaleRequest);
            return;
        }
        self.state_request = Some(request);
        self.state_watch = None;
        let Some(target) = target else {
            (self.sink)(ToIde::StateObservation(StateReply::Stopped { request }));
            return;
        };
        match self.start_state_watch(request, target) {
            Ok((index, watch)) => {
                self.state_watch = Some(watch);
                self.scenes[index].dirty = true;
            }
            Err(reason) => self.state_refused(request, reason),
        }
    }
    fn start_state_watch(
        &self,
        request: StateRequestId,
        target: StateTarget,
    ) -> Result<(usize, StateWatch), StateRefusal> {
        let index = self.state_scene(&target)?;
        let scene = &self.scenes[index];
        let last = scene.last.as_ref().ok_or(StateRefusal::NotPublished)?;
        if !last
            .views
            .iter()
            .any(|view| view.mounted == Some(target.mounted))
        {
            return Err(StateRefusal::ViewMissing);
        }
        let reading = scene
            .runtime
            .sites()
            .into_iter()
            .find(|site| {
                site.mounted
                    .is_some_and(|mounted| mounted.get() == target.mounted.get())
            })
            .ok_or(StateRefusal::ViewMissing)?
            .reading;
        Ok((
            index,
            StateWatch {
                request,
                target,
                scene: Rc::clone(&scene.name),
                reading,
            },
        ))
    }
    fn state_refused(&mut self, request: StateRequestId, reason: StateRefusal) {
        (self.sink)(ToIde::StateObservation(StateReply::Refused {
            request,
            reason,
        }));
    }
    fn revoke_state(&mut self, reason: StateRefusal) {
        if let Some(watch) = self.state_watch.take() {
            self.state_refused(watch.request, reason);
        }
    }
    pub(super) fn check_state_watch(&mut self) {
        let Some(watch) = &self.state_watch else {
            return;
        };
        let validity = self.state_scene(&watch.target).and_then(|index| {
            if self.scenes[index].name == watch.scene {
                Ok(())
            } else {
                Err(StateRefusal::ViewMissing)
            }
        });
        if let Err(reason) = validity {
            self.revoke_state(reason);
        }
    }
    pub(super) fn state_changed(&self) -> bool {
        let Some(watch) = &self.state_watch else {
            return false;
        };
        let Ok(index) = self.state_scene(&watch.target) else {
            return true;
        };
        self.scenes[index].last.as_ref().is_none_or(|last| {
            last.state.as_ref() != Some(&(watch.request, read(watch.reading.as_ref())))
        })
    }
    pub(super) fn state_scene_index(&self) -> Option<usize> {
        self.state_watch
            .as_ref()
            .and_then(|watch| self.state_scene(&watch.target).ok())
    }
    pub(super) fn state_for_pass(
        &mut self,
        index: usize,
        sites: &[SitePlacement],
    ) -> Option<(StateRequestId, StateRead)> {
        if self.state_scene_index() != Some(index) {
            return None;
        }
        let watch = self.state_watch.as_mut()?;
        let Some(site) = sites.iter().find(|site| {
            site.mounted
                .is_some_and(|mounted| mounted.get() == watch.target.mounted.get())
        }) else {
            self.revoke_state(StateRefusal::ViewMissing);
            return None;
        };
        watch.reading = site.reading.clone();
        Some((watch.request, read(watch.reading.as_ref())))
    }
    pub(super) fn publish_state(&mut self, seq: u64, state: &Option<(StateRequestId, StateRead)>) {
        if let (Some(watch), Some((request, read))) = (&self.state_watch, state) {
            (self.sink)(ToIde::StateObservation(StateReply::Snapshot(
                wire::StateSnapshot {
                    request: *request,
                    target: watch.target.clone(),
                    seq,
                    read: read.clone(),
                },
            )));
        }
    }
}

fn read(binding: Option<&ReadBinding>) -> StateRead {
    let Some(binding) = binding else {
        return StateRead::NoBinding;
    };
    match binding.observe() {
        Ok(Some(deps)) => deps
            .into_iter()
            .map(convert_dependency)
            .collect::<Result<Vec<_>, _>>()
            .map_or(StateRead::Limit, StateRead::Values),
        Ok(None) => StateRead::UnavailableBinding,
        Err(observation::Unavailable::Borrowed) => StateRead::Borrowed,
        Err(observation::Unavailable::TooManyDependencies) => StateRead::Limit,
    }
}
fn convert_dependency(dep: observation::Observation) -> Result<wire::StateDependency, StateRead> {
    let key = match dep.dependency {
        observation::DepKey::State(id) => wire::DependencyKey::State(id),
        observation::DepKey::Store(id) => wire::DependencyKey::Store(id),
    };
    let evidence = match dep.evidence {
        observation::Evidence::State(state) => {
            if state.type_name.len() > wire::STATE_TEXT_LIMIT
                || state.constructor.file().len() > wire::STATE_TEXT_LIMIT
            {
                return Err(StateRead::Limit);
            }
            wire::StateEvidence::State {
                type_name: state.type_name.to_owned(),
                constructor: wire::StateConstructor {
                    file: state.constructor.file().to_owned(),
                    line: state.constructor.line(),
                    column: state.constructor.column(),
                },
                version: state.version,
                value: match state.value {
                    observation::Value::Signed(n) => wire::StateValue::Signed(n),
                    observation::Value::Unsigned(n) => wire::StateValue::Unsigned(n),
                    observation::Value::Float(n) => wire::StateValue::FloatBits(n.to_bits()),
                    observation::Value::Bool(n) => wire::StateValue::Bool(n),
                    observation::Value::Char(n) => wire::StateValue::Char(n),
                    observation::Value::Text(n) => wire::StateValue::Text(n),
                    observation::Value::Opaque => wire::StateValue::Opaque,
                    observation::Value::TooLarge { bytes } => wire::StateValue::TooLarge {
                        bytes: bytes as u64,
                    },
                    observation::Value::Updating => wire::StateValue::Updating,
                },
            }
        }
        observation::Evidence::Missing => wire::StateEvidence::Missing,
        observation::Evidence::Unsupported => wire::StateEvidence::Unsupported,
    };
    Ok(wire::StateDependency { key, evidence })
}
