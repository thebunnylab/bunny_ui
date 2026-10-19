//! The view that shows a world: `scene_view(world, camera)`.
//!
//! It is a box the framework owns: it paints the world, turns the camera
//! under the hand when asked to, reports what a tap and the pointer
//! land on, and writes labels that follow the camera. The labels are
//! painted with the picture, so a camera that moves repaints them and no
//! body runs.

use std::rc::Rc;
use std::sync::Arc;

use motor::state::State;

use super::camera::Camera;
use super::frame::WorldStyle;
use super::math::{Vec3, vec3};
use super::orbit::{Orbit, OrbitLimits};
use super::pick::Hit;
use super::store::{Entity, World};
use super::view::CameraSource;
use crate::custom::{CustomElement, CustomView, ElementEvent, EventCtx, PaintCtx, Painter, Response, custom};
use crate::layout::{Color, Point, Rect, Size};
use crate::view::{Component, View};

/// Where a label stands in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    Point(Vec3),
    /// Over the top of an entity, at the middle of its box.
    Above(Entity),
    /// The middle of an entity's box.
    Center(Entity),
}

impl From<Vec3> for Anchor {
    fn from(point: Vec3) -> Anchor {
        Anchor::Point(point)
    }
}

impl From<Entity> for Anchor {
    fn from(entity: Entity) -> Anchor {
        Anchor::Above(entity)
    }
}

impl Anchor {
    /// The world point, or `None` for an entity that shows nothing.
    pub fn point(&self) -> Option<Vec3> {
        let bounds = match self {
            Anchor::Point(point) => return Some(*point),
            Anchor::Above(entity) | Anchor::Center(entity) => entity.bounds(),
        };
        if bounds.is_empty() {
            return None;
        }
        let center = bounds.center();
        Some(match self {
            Anchor::Above(_) => vec3(center.x, bounds.max.y, center.z),
            _ => center,
        })
    }
}

type HitHandler = Rc<dyn Fn(Option<Hit>)>;
type AnchorSource = Rc<dyn Fn() -> Option<Anchor>>;

/// A world seen through a camera, in a box. Build it with [`scene_view`].
#[derive(Clone)]
pub struct SceneView<C: CameraSource> {
    world: World,
    camera: C,
    orbit: Option<(Orbit, State<Camera>)>,
    on_tap: Option<HitHandler>,
    on_hover: Option<HitHandler>,
    style: WorldStyle,
    labels: Vec<(AnchorSource, Arc<str>)>,
}

/// Shows `world` through `camera`: a fixed `Camera`, or a `State<Camera>`
/// the hand can move with [`SceneView::orbit`].
///
/// ```ignore
/// scene_view(self.world, self.camera)
///     .orbit()
///     .on_tap(move |hit| self.picked.set(hit.map(|hit| hit.entity)))
///     .label(move || self.picked.get(), "Selected")
/// ```
pub fn scene_view<C: CameraSource>(world: World, camera: C) -> SceneView<C> {
    SceneView { world, camera, orbit: None, on_tap: None, on_hover: None, style: WorldStyle::default(), labels: Vec::new() }
}

impl SceneView<State<Camera>> {
    /// The hand moves the camera: a drag turns the view, shift and a
    /// drag pan it, the wheel and a pinch zoom toward the pointer. Only a
    /// camera in a `State` can move — a fixed camera has no orbit:
    ///
    /// ```compile_fail
    /// use bunny_ui::world::{Camera, Vec3, World, scene_view};
    /// let world = World::new();
    /// let _ = scene_view(world, Camera::looking_at(Vec3::Z, Vec3::ZERO)).orbit();
    /// ```
    pub fn orbit(self) -> Self {
        let camera = self.camera;
        SceneView { orbit: Some((Orbit::new(), camera)), ..self }
    }

    /// [`SceneView::orbit`], held inside `limits`.
    pub fn orbit_limits(self, limits: OrbitLimits) -> Self {
        let camera = self.camera;
        SceneView { orbit: Some((Orbit::new().limits(limits), camera)), ..self }
    }
}

impl<C: CameraSource> SceneView<C> {
    /// Called with what a tap landed on — `None` for empty space.
    pub fn on_tap(self, action: impl Fn(Option<Hit>) + 'static) -> Self {
        SceneView { on_tap: Some(Rc::new(action)), ..self }
    }

    /// Called with what the pointer is over as it moves, and `None` when
    /// it leaves the box.
    pub fn on_hover(self, action: impl Fn(Option<Hit>) + 'static) -> Self {
        SceneView { on_hover: Some(Rc::new(action)), ..self }
    }

    pub fn style(self, style: WorldStyle) -> Self {
        SceneView { style, ..self }
    }

    /// A label over the point `at` names, while it names one. The label
    /// follows the camera; a point behind the eye or outside the box
    /// shows none.
    pub fn label<A: Into<Anchor>>(mut self, at: impl Fn() -> Option<A> + 'static, text: impl Into<Arc<str>>) -> Self {
        self.labels.push((Rc::new(move || at().map(Into::into)), text.into()));
        self
    }
}

impl<C: CameraSource> Component for SceneView<C> {
    fn body(self) -> impl View {
        let element: CustomView = custom(SceneElement {
            world: self.world,
            camera: self.camera,
            orbit: self.orbit,
            on_tap: self.on_tap,
            on_hover: self.on_hover,
            style: self.style,
            labels: self.labels,
        });
        element
    }
}

/// The box behind a [`SceneView`].
struct SceneElement<C: CameraSource> {
    world: World,
    camera: C,
    orbit: Option<(Orbit, State<Camera>)>,
    on_tap: Option<HitHandler>,
    on_hover: Option<HitHandler>,
    style: WorldStyle,
    labels: Vec<(AnchorSource, Arc<str>)>,
}

/// The pill a label sits on.
const LABEL_INK: Color = Color { r: 255, g: 255, b: 255, a: 255 };
const LABEL_FILL: Color = Color { r: 24, g: 26, b: 32, a: 220 };
const LABEL_PAD: f64 = 6.0;
/// How far above its point a label stands.
const LABEL_LIFT: f64 = 8.0;

impl<C: CameraSource> SceneElement<C> {
    fn pick(&self, at: Point, size: Size) -> Option<Hit> {
        self.world.pick(self.camera, at, size)
    }

    fn tap(&self, at: Point, size: Size) {
        if let Some(action) = &self.on_tap {
            action(self.pick(at, size));
        }
    }
}

impl<C: CameraSource> CustomElement for SceneElement<C> {
    fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
        painter.world_styled(ctx.bounds(), self.world, self.camera, &self.style);
        if self.labels.is_empty() {
            return;
        }
        let camera = self.camera.camera();
        let size = ctx.size();
        for (at, text) in &self.labels {
            let Some(point) = at().and_then(|anchor| anchor.point()) else { continue };
            let Some(spot) = camera.project(point, size) else { continue };
            if spot.x < 0.0 || spot.y < 0.0 || spot.x > size.width || spot.y > size.height {
                continue;
            }
            let line = ctx.metrics.line(text);
            let (width, height) = (line.width + LABEL_PAD * 2.0, line.height() + LABEL_PAD);
            let rect = Rect {
                origin: Point { x: spot.x - width * 0.5, y: spot.y - LABEL_LIFT - height },
                size: Size { width, height },
            };
            painter.fill_rounded(rect, LABEL_FILL, height * 0.5);
            painter.text(Point { x: rect.origin.x + LABEL_PAD, y: rect.origin.y + LABEL_PAD * 0.5 }, Arc::clone(text), LABEL_INK);
        }
    }

    fn takes_drag(&self) -> bool {
        self.orbit.is_some()
    }

    /// The closed hand while a drag moves the camera; the platform's own
    /// pointer otherwise.
    fn cursor(&self, _at: Point, _visible: Rect) -> Option<crate::layout::Cursor> {
        let dragging = self.orbit.as_ref().is_some_and(|(orbit, _)| orbit.is_dragging());
        dragging.then_some(crate::layout::Cursor::Grabbing)
    }

    fn event(&self, event: &ElementEvent, ctx: &EventCtx) -> Response {
        let size = ctx.size();
        if let ElementEvent::PointerMoved { at, pressed: false, .. } = event
            && let Some(action) = &self.on_hover
        {
            action(self.pick(*at, size));
        }
        if let (ElementEvent::PointerExited, Some(action)) = (event, &self.on_hover) {
            action(None);
        }
        match &self.orbit {
            Some((orbit, camera)) => {
                let response = orbit.handle(event, size, *camera, self.world);
                // a release the orbit did not use ends a press that never travelled
                if let ElementEvent::PointerUp { at } = event
                    && !response.handled
                {
                    self.tap(*at, size);
                    return Response::handled();
                }
                response
            }
            None => match event {
                ElementEvent::PointerDown { .. } => Response::handled(),
                ElementEvent::PointerUp { at } => {
                    self.tap(*at, size);
                    Response::handled()
                }
                _ => Response::ignored(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::DrawCommand;
    use crate::runtime::Runtime;
    use crate::world::material::Material;
    use crate::world::mesh::Mesh;
    use std::cell::Cell;

    const SIZE: Size = Size { width: 300.0, height: 200.0 };

    thread_local! {
        static BODIES: Cell<u32> = const { Cell::new(0) };
    }

    #[derive(Clone, Copy)]
    struct Viewer {
        world: World,
        camera: State<Camera>,
        picked: State<Option<Entity>>,
    }

    impl Component for Viewer {
        fn body(self) -> impl View {
            BODIES.with(|bodies| bodies.set(bodies.get() + 1));
            scene_view(self.world, self.camera)
                .orbit()
                .on_tap(move |hit| self.picked.set(hit.map(|hit| hit.entity)))
                .label(move || self.picked.with(|picked| *picked), "Selected")
        }
    }

    fn mount() -> (Runtime, Viewer, Entity) {
        let world = World::new();
        let cube = world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::rgb(220, 120, 40)));
        let viewer = Viewer {
            world,
            camera: State::new(Camera::looking_at(vec3(0.0, 0.0, 6.0), Vec3::ZERO)),
            picked: State::new(None),
        };
        let runtime = Runtime::new();
        let _ = runtime.display_frame(&viewer, SIZE);
        let _ = runtime.display_frame(&viewer, SIZE);
        BODIES.with(|bodies| bodies.set(0));
        (runtime, viewer, cube)
    }

    fn world_generation(list: &crate::layout::DisplayList) -> u64 {
        list.iter()
            .find_map(|command| match command {
                DrawCommand::Image { source: crate::image_engine::ImageSource::World { generation, .. }, .. } => Some(*generation),
                _ => None,
            })
            .expect("a world")
    }

    #[test]
    fn a_tap_reports_the_entity_under_the_pointer() {
        let (runtime, viewer, cube) = mount();
        runtime.pointer_pressed(150.0, 100.0);
        runtime.pointer_released(150.0, 100.0);
        assert_eq!(viewer.picked.with(|picked| *picked), Some(cube));
        runtime.pointer_pressed(10.0, 10.0);
        runtime.pointer_released(10.0, 10.0);
        assert_eq!(viewer.picked.with(|picked| *picked), None, "empty space");
    }

    #[test]
    fn a_drag_orbits_and_runs_no_body() {
        let (runtime, viewer, _) = mount();
        let before = world_generation(&runtime.display_frame(&viewer, SIZE));
        let camera = viewer.camera.with(|camera| *camera);
        runtime.pointer_pressed(150.0, 100.0);
        for x in [160.0, 180.0, 200.0] {
            runtime.pointer_moved(x, 100.0, crate::action::Modifiers::NONE);
        }
        runtime.pointer_released(200.0, 100.0);
        assert_ne!(viewer.camera.with(|camera| *camera), camera, "the camera turned");
        let after = world_generation(&runtime.display_frame(&viewer, SIZE));
        assert_ne!(before, after);
        assert_eq!(viewer.picked.with(|picked| *picked), None, "a drag is no tap");
        assert_eq!(BODIES.with(Cell::get), 0, "no body ran");
    }

    #[test]
    fn a_label_follows_the_camera_without_a_body() {
        let (runtime, viewer, cube) = mount();
        viewer.picked.set(Some(cube));
        let label = |list: &crate::layout::DisplayList| {
            list.iter().find_map(|command| match command {
                DrawCommand::TextLine { origin, content, .. } if &**content == "Selected" => Some(*origin),
                _ => None,
            })
        };
        let first = label(&runtime.display_frame(&viewer, SIZE)).expect("the label shows");
        BODIES.with(|bodies| bodies.set(0));
        viewer.camera.set(Camera::looking_at(vec3(2.0, 1.0, 6.0), Vec3::ZERO));
        let second = label(&runtime.display_frame(&viewer, SIZE)).expect("still there");
        assert_ne!(first, second, "it moved with the camera");
        assert_eq!(BODIES.with(Cell::get), 0, "and no body ran for it");
        // behind the eye there is no label
        viewer.camera.set(Camera::looking_at(vec3(0.0, 0.0, 6.0), vec3(0.0, 0.0, 12.0)));
        assert_eq!(label(&runtime.display_frame(&viewer, SIZE)), None);
    }

    #[test]
    fn a_fixed_camera_paints_and_taps() {
        let world = World::new();
        let cube = world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::WHITE));
        let tapped = State::new(None::<Entity>);
        let view = scene_view(world, Camera::looking_at(vec3(0.0, 0.0, 6.0), Vec3::ZERO))
            .on_tap(move |hit| tapped.set(hit.map(|hit| hit.entity)));
        let runtime = Runtime::new();
        let _ = runtime.display_frame(&view, SIZE);
        runtime.pointer_pressed(150.0, 100.0);
        runtime.pointer_released(150.0, 100.0);
        assert_eq!(tapped.with(|t| *t), Some(cube));
    }
}
