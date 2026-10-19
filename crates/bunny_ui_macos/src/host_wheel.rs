//! Keep an embedded document's wheel gesture with its chosen scroll owner.
//!
//! ## Wiring
//! The native host proxy passes the original NSEvent unchanged. This pure
//! state decides only its destination; zero-delta bookends reach both owners.

use bunny_ui::custom::WheelPhase;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// The recipients of one original native wheel event.
pub(crate) enum Target {
    /// Horizontal document motion.
    Document,
    /// Vertical motion in the enclosing reader.
    Parent,
    /// A zero-delta gesture bookend for both owners.
    Both,
}

#[derive(Default)]
/// Axis ownership for one trackpad gesture, independent of native pointers.
pub(crate) struct ParentWheel {
    owner: Option<Target>,
}

impl ParentWheel {
    /// Choose the target without transforming its native event.
    pub(crate) fn route(
        &mut self,
        dx: f64,
        dy: f64,
        phase: WheelPhase,
        continuous: bool,
    ) -> Target {
        if matches!(phase, WheelPhase::Began) || !continuous {
            self.owner = None;
        }
        let target = if dx == 0.0 && dy == 0.0 {
            // Both destinations observe native gesture bookends even before
            // the first nonzero delta establishes the axis. Neither moves.
            Target::Both
        } else {
            *self.owner.get_or_insert(if dy.abs() >= dx.abs() {
                Target::Parent
            } else {
                Target::Document
            })
        };
        // AppKit ends the finger phase before starting momentum. Keep its
        // owner until a new physical gesture or independent wheel step.
        if !continuous {
            self.owner = None;
        }
        target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_turns_reach_the_parent_and_horizontal_turns_keep_the_document() {
        let mut route = ParentWheel::default();
        assert_eq!(
            route.route(0.0, -40.0, WheelPhase::Began, true),
            Target::Parent
        );
        assert_eq!(
            route.route(-40.0, 0.0, WheelPhase::Began, true),
            Target::Document
        );
    }

    #[test]
    fn a_trackpad_gesture_retains_its_axis_despite_later_diagonal_motion() {
        let mut route = ParentWheel::default();
        assert_eq!(route.route(0.0, 0.0, WheelPhase::Began, true), Target::Both);
        assert_eq!(
            route.route(1.0, -40.0, WheelPhase::Changed, true),
            Target::Parent
        );
        assert_eq!(
            route.route(20.0, -1.0, WheelPhase::Changed, true),
            Target::Parent
        );
        assert_eq!(route.route(0.0, 0.0, WheelPhase::Ended, true), Target::Both);
        assert_eq!(
            route.route(40.0, 1.0, WheelPhase::Began, true),
            Target::Document
        );
    }

    #[test]
    fn momentum_keeps_the_owner_chosen_before_the_finger_lift() {
        let mut route = ParentWheel::default();
        route.route(0.0, -40.0, WheelPhase::Began, true);
        route.route(0.0, 0.0, WheelPhase::Ended, true);
        assert_eq!(
            route.route(12.0, -1.0, WheelPhase::Changed, true),
            Target::Parent
        );
    }

    #[test]
    fn a_notched_wheel_does_not_retain_a_previous_trackpad_owner() {
        let mut route = ParentWheel::default();
        route.route(-40.0, 0.0, WheelPhase::Began, true);
        assert_eq!(
            route.route(0.0, -3.0, WheelPhase::Changed, false),
            Target::Parent
        );
        assert_eq!(
            route.route(-3.0, 0.0, WheelPhase::Changed, false),
            Target::Document
        );
    }
}
