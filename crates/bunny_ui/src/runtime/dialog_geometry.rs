//! Bind persistent controllers to the structural paths of live dialog presentations.
//! Hosts enqueue observations; application callbacks never run during layout.

use super::Runtime;
use crate::window::{
    DialogGeometry, DialogGeometryObservation, DialogGeometrySession, WindowError,
};
use motor::hash::FxHashMap;
use std::{cell::RefCell, rc::Rc};

type Dispatch = Rc<dyn Fn(DialogGeometryObservation)>;

struct Binding {
    controller: DialogGeometry,
    session: Option<Rc<DialogGeometrySession>>,
}

impl Drop for Binding {
    fn drop(&mut self) {
        if let Some(session) = &self.session {
            session.retire();
        }
    }
}

#[derive(Default)]
pub(super) struct Registry {
    bindings: RefCell<FxHashMap<String, Binding>>,
    dispatch: RefCell<Option<Dispatch>>,
}

impl Registry {
    pub(super) fn sync<'a>(&self, requested: impl Iterator<Item = (&'a str, &'a DialogGeometry)>) {
        let requested: Vec<_> = requested.collect();
        let native = self.dispatch.borrow().is_some();
        let mut observations = Vec::new();
        {
            let mut bindings = self.bindings.borrow_mut();
            // Retire the whole departing set before acquiring its replacements.
            // Moving a controller to another path must not depend on map order.
            bindings.retain(|path, binding| {
                requested
                    .iter()
                    .any(|(asked, controller)| *asked == path && **controller == binding.controller)
            });
            for (path, controller) in requested {
                if bindings.contains_key(path) {
                    continue;
                }
                let session = match controller.begin() {
                    Ok(session) => {
                        if !native {
                            observations.extend(session.capture(Err(WindowError::Unsupported(
                                "this host presents dialogs inside the scene; native geometry is unavailable",
                            ))));
                        }
                        Some(session)
                    }
                    Err(error) => {
                        observations.extend(controller.capture_error(error));
                        None
                    }
                };
                bindings.insert(
                    path.to_owned(),
                    Binding {
                        controller: controller.clone(),
                        session,
                    },
                );
            }
        }
        let dispatch = self.dispatch.borrow().clone();
        for observation in observations {
            if let Some(dispatch) = &dispatch {
                dispatch(observation);
            } else {
                // The next executor turn is outside the current layout/paint borrow.
                motor::task::spawn(async move {
                    observation.deliver();
                })
                .detach();
            }
        }
    }
}

impl Runtime {
    /// Installs the native shell's enqueue-only geometry dispatcher before its first frame.
    /// The function must defer delivery until platform and renderer borrows are released.
    #[doc(hidden)]
    pub fn set_dialog_geometry_dispatch(
        &self,
        dispatch: impl Fn(DialogGeometryObservation) + 'static,
    ) {
        self.retire_dialog_geometry();
        *self.dialog_geometry.dispatch.borrow_mut() = Some(Rc::new(dispatch));
    }

    /// The lease of this actual overlay presentation, never an ordinary `WindowId`.
    #[doc(hidden)]
    pub fn dialog_geometry_session(&self, path: &str) -> Option<Rc<DialogGeometrySession>> {
        self.dialog_geometry
            .bindings
            .borrow()
            .get(path)
            .and_then(|binding| binding.session.clone())
    }

    /// Retires every dialog before the parent is destroyed, even if its runtime survives.
    #[doc(hidden)]
    pub fn retire_dialog_geometry(&self) {
        self.dialog_geometry.bindings.borrow_mut().clear();
    }
}
