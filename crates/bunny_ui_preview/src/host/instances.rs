//! Each instance owns its runtime and provisional override context.
//!
//! ## Wiring
//! New wire messages enter here; the legacy host lane keeps its addressing.
//! Selection is validated before replacing any runtime. Delayed snapshots carry
//! the runtime identity so a removed/recreated artboard cannot inherit a request.

use super::*;
use wire::{InstanceCommand, InstanceSelection, SceneInstanceId};

pub(super) enum SceneLane {
    Legacy,
    Instances,
}
impl SceneLane {
    pub(super) fn contains(&self, scene: &Scene) -> bool {
        match self {
            Self::Legacy => scene.instance().is_none(),
            Self::Instances => scene.instance().is_some(),
        }
    }
}

pub(super) enum SceneMode {
    Legacy,
    Instance {
        id: SceneInstanceId,
        overrides: provenance::OverrideSet,
    },
}

impl Scene {
    pub(super) fn instance(&self) -> Option<&SceneInstanceId> {
        match &self.mode {
            SceneMode::Legacy => None,
            SceneMode::Instance { id, .. } => Some(id),
        }
    }

    pub(super) fn override_set(&self) -> Option<&provenance::OverrideSet> {
        match &self.mode {
            SceneMode::Legacy => None,
            SceneMode::Instance { overrides, .. } => Some(overrides),
        }
    }
    pub(super) fn needs_frame(&self) -> bool {
        self.execution.with(|| match self.override_set() {
            Some(scope) => scope.with(|| self.runtime.needs_frame()),
            None => self.runtime.needs_frame(),
        })
    }

    fn refresh_overrides(&mut self) {
        if let SceneMode::Instance { overrides, .. } = &mut self.mode {
            *overrides =
                provenance::OverrideSet::new(self.overrides.iter().map(|(site, value)| {
                    (
                        SiteKey {
                            file: site.file.clone(),
                            line: site.line,
                            column: site.column,
                        },
                        value.clone(),
                    )
                }));
        }
        self.dirty = true;
    }
}

impl Host {
    pub(super) fn execution_for(
        &self,
        message: &ToHost,
    ) -> Option<bunny_ui_core::preview::ExecutionContext> {
        let scene = match message {
            ToHost::Instance {
                instance, variant, ..
            }
            | ToHost::SetExecution {
                instance, variant, ..
            } => self
                .scenes
                .iter()
                .find(|scene| scene.instance() == Some(instance) && scene.variant == *variant),
            ToHost::Pointer { id, .. }
            | ToHost::Wheel { id, .. }
            | ToHost::Touch { id, .. }
            | ToHost::Magnify { id, .. }
            | ToHost::Key { id, .. }
            | ToHost::Text { id, .. } => self
                .scenes
                .iter()
                .find(|scene| scene.instance().is_none() && scene.id == *id),
            _ => None,
        }?;
        Some(scene.execution.context())
    }

    pub(super) fn set_execution(
        &mut self,
        instance: SceneInstanceId,
        variant: u32,
        mode: wire::ExecutionMode,
    ) {
        let Some(scene) = self
            .scenes
            .iter_mut()
            .find(|scene| scene.instance() == Some(&instance) && scene.variant == variant)
        else {
            self.instance_error(instance, Some(variant), "instance variant is not selected");
            return;
        };
        let mode = match mode {
            wire::ExecutionMode::Design => ExecutionMode::Design,
            wire::ExecutionMode::Play => ExecutionMode::Play,
        };
        scene.dirty |= scene.execution.set_mode(mode);
    }

    fn instance_error(&mut self, instance: SceneInstanceId, variant: Option<u32>, message: &str) {
        (self.sink)(ToIde::InstanceError {
            instance,
            variant,
            message: message.to_owned(),
        });
    }

    pub(super) fn select_instances(&mut self, selections: Vec<InstanceSelection>) {
        let mut identities = HashSet::new();
        for selection in &selections {
            if !identities.insert(&selection.instance) {
                self.instance_error(
                    selection.instance.clone(),
                    None,
                    "duplicate instance identity",
                );
                return;
            }
            if preview_entry(&selection.ident).is_err() {
                self.instance_error(
                    selection.instance.clone(),
                    None,
                    "preview is not exported by this build",
                );
                return;
            }
        }
        self.select_scenes(
            SceneLane::Instances,
            selections
                .into_iter()
                .map(|selection| {
                    let legacy = wire::Selection {
                        id: selection.instance.to_string(),
                        ident: selection.ident,
                        traits: selection.traits,
                    };
                    (legacy, Some(selection.instance))
                })
                .collect(),
        );
    }

    pub(super) fn instance_command(
        &mut self,
        instance: SceneInstanceId,
        variant: u32,
        command: InstanceCommand,
    ) {
        let Some(index) = self
            .scenes
            .iter()
            .position(|scene| scene.instance() == Some(&instance) && scene.variant == variant)
        else {
            self.instance_error(instance, Some(variant), "instance variant is not selected");
            return;
        };
        if matches!(command, InstanceCommand::RequestSnapshot) {
            if self.pictures.as_ref().is_some_and(|door| door.busy()) {
                self.held_instances.push((
                    instance,
                    variant,
                    Rc::clone(&self.scenes[index].name),
                    Instant::now() + self.patience,
                ));
            } else {
                self.snapshot(index);
            }
            return;
        }
        // The immutable context is held separately while the scene mutates its
        // runtime or replaces its next override set.
        let scope = self.scenes[index].override_set().cloned();
        if let Some(scope) = scope {
            scope.with(|| self.apply_instance_command(index, command));
        }
    }

    fn apply_instance_command(&mut self, index: usize, command: InstanceCommand) {
        let scene = &mut self.scenes[index];
        match command {
            InstanceCommand::Retrait(traits) => {
                let variants = convert::core_traits(&traits).variants();
                // A command changes one existing variant, never creates siblings.
                if let Some(traits) = variants
                    .get(scene.variant as usize)
                    .or_else(|| variants.first())
                {
                    traits.apply(&scene.runtime);
                    scene.traits = traits.clone();
                    scene.resized = None;
                    scene.dirty = true;
                }
            }
            InstanceCommand::Resize(size) => {
                scene.resized = Some(size.into());
                scene.dirty = true;
            }
            InstanceCommand::Pointer {
                kind,
                x,
                y,
                modifiers,
            } => {
                scene.dirty |= match kind {
                    wire::PointerKind::Move => {
                        scene.runtime.pointer_moved(x, y, modifiers_of(modifiers))
                    }
                    wire::PointerKind::Down => scene.runtime.pointer_pressed(x, y),
                    wire::PointerKind::Up => scene.runtime.pointer_released(x, y).is_some(),
                    wire::PointerKind::Leave => scene.runtime.pointer_exited(),
                };
            }
            InstanceCommand::Wheel { x, y, dx, dy } => {
                scene.dirty |= scene.runtime.wheel(x, y, dx, dy);
            }
            InstanceCommand::Touch {
                phase,
                touch,
                x,
                y,
                taps,
            } => {
                scene.dirty |= match phase {
                    wire::TouchPhase::Began => scene.runtime.touch_began(touch, x, y, taps),
                    wire::TouchPhase::Moved => scene.runtime.touch_moved(touch, x, y),
                    wire::TouchPhase::Ended => scene.runtime.touch_ended(touch, x, y),
                    wire::TouchPhase::Cancelled => scene.runtime.touch_cancelled(touch),
                };
            }
            InstanceCommand::Magnify { x, y, scale } => {
                scene.dirty |= scene.runtime.magnify(x, y, scale);
            }
            InstanceCommand::Key { key, modifiers } => {
                if let Some(stroke) = stroke_named(&key, modifiers) {
                    scene.dirty |= scene.runtime.key_stroke(stroke).handled;
                }
            }
            InstanceCommand::Text { text } => {
                for typed in text.chars() {
                    let stroke = bunny_ui_core::action::Stroke {
                        pattern: KeyPattern::key(Key::Char(typed)),
                        typed: Some(typed),
                    };
                    scene.dirty |= scene.runtime.key_stroke(stroke).handled;
                }
            }
            InstanceCommand::Override { site, op, patch } => {
                Self::instance_override(scene, site, as_override(op, convert::modifier(patch)))
            }
            InstanceCommand::Place { site, op, patch } => Self::instance_override(
                scene,
                site,
                as_override(op, convert::place_modifier(patch)),
            ),
            InstanceCommand::ClearOverrides => {
                scene.overrides.clear();
                scene.refresh_overrides();
            }
            // Handled before borrowing the scene, because a snapshot uses the engines.
            InstanceCommand::RequestSnapshot => {}
        }
    }

    fn instance_override(scene: &mut Scene, site: wire::SiteKey, value: Override) {
        scene
            .overrides
            .retain(|(standing, old)| !(standing == &site && supersedes(&value, old)));
        scene.overrides.push((site, value));
        scene.refresh_overrides();
    }

    pub(super) fn answer_held_instances(&mut self) {
        let busy = self.pictures.as_ref().is_some_and(|door| door.busy());
        let mut pending = Vec::new();
        for (instance, variant, name, deadline) in std::mem::take(&mut self.held_instances) {
            let index = self.scenes.iter().position(|scene| {
                scene.name == name
                    && scene.instance() == Some(&instance)
                    && scene.variant == variant
            });
            match index {
                None => self.instance_error(
                    instance,
                    Some(variant),
                    "instance was removed before its snapshot completed",
                ),
                Some(_) if busy && Instant::now() < deadline => {
                    pending.push((instance, variant, name, deadline))
                }
                Some(index) => self.snapshot(index),
            }
        }
        self.held_instances = pending;
    }
}
