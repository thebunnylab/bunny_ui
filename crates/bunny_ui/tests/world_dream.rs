//! The two dream snippets of the world API, compiled and exercised: a
//! viewer in a few lines, and the full door of a tool-driven editor.

#![cfg(feature = "3d")]

use bunny_ui::custom::{CustomElement, ElementEvent, EventCtx, PaintCtx, Painter, Response, custom};
use bunny_ui::layout::{Color, Rect, Size};
use bunny_ui::prelude::*;
use bunny_ui::runtime::Runtime;
use bunny_ui::world::{Camera, Entity, Hit, Light, Material, Mesh, Orbit, World, scene_view, vec3};

const SIZE: Size = Size { width: 400.0, height: 300.0 };

// MARK: - A — a viewer

#[derive(Clone, Copy)]
struct Viewer {
    world: World,
    camera: State<Camera>,
    picked: State<Option<Entity>>,
}

fn viewer() -> Viewer {
    let world = World::new();
    world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::hex(0xE4572E)));
    world.add(Mesh::sphere(0.5), Material::color(Color::hex(0x2E86AB))).set_position(vec3(1.5, 0.0, 0.0));
    world.add(Mesh::plane(8.0, 8.0), Material::color(Color::hex(0xF2F3F7))).set_position(vec3(0.0, -0.5, 0.0));
    world.add_light(Light::sun().toward(vec3(-1.0, -2.0, -1.5)).with_shadows());
    Viewer {
        world,
        camera: State::new(Camera::looking_at(vec3(4.0, 3.0, 5.0), vec3(0.0, 0.0, 0.0))),
        picked: State::new(None),
    }
}

impl Component for Viewer {
    fn body(self, _ctx: &Context) -> impl View {
        scene_view(self.world, self.camera)
            .orbit()
            .on_tap(move |hit| self.picked.set(hit.map(|hit| hit.entity)))
            .label(move || self.picked.get(), "Selected")
    }
}

#[test]
fn the_viewer_snippet_compiles_and_reacts() {
    let page = viewer();
    let runtime = Runtime::new();
    let _ = runtime.display_frame(&page, SIZE);
    let target = page.camera.get().project(vec3(0.0, 0.0, 0.0), SIZE).expect("the cube is in view");
    runtime.pointer_pressed(target.x, target.y);
    runtime.pointer_released(target.x, target.y);
    assert!(page.picked.get().is_some(), "the tap found the cube");
}

// MARK: - B — the full door: a tool-driven editor

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Select,
    Push,
}

#[derive(Clone, Copy)]
struct Editor {
    world: World,
    camera: State<Camera>,
    orbit: Orbit,
    tool: State<Tool>,
    under: State<Option<Hit>>,
}

impl CustomElement for Editor {
    fn accepts_keys(&self) -> bool {
        true
    }

    fn takes_drag(&self) -> bool {
        true
    }

    fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
        // the picture first, then the marks over it, in the same frame
        painter.world(ctx.bounds(), self.world, self.camera);
        if let Some(hit) = self.under.get()
            && let Some(at) = self.camera.get().project(hit.point, ctx.size())
        {
            painter.fill_rounded(Rect::around(at, 8.0, 8.0), Color::hex(0x3B82F6), 4.0);
        }
    }

    fn event(&self, event: &ElementEvent, ctx: &EventCtx) -> Response {
        match event {
            ElementEvent::PointerMoved { at, pressed: false, .. } => {
                self.under.set(self.world.pick(self.camera, *at, ctx.size()));
                Response::handled()
            }
            ElementEvent::PointerDown { .. } if self.tool.get() == Tool::Push => {
                if let Some(hit) = self.under.get() {
                    hit.entity.set_position(hit.entity.position() + hit.normal * 0.25);
                }
                Response::handled()
            }
            ElementEvent::Key(stroke) if stroke.typed == Some('p') => {
                self.tool.set(Tool::Push);
                Response::handled()
            }
            // what the tool leaves moves the camera: orbit, pan, zoom to the cursor
            _ => self.orbit.event(event, ctx, self.camera, self.world),
        }
    }
}

#[derive(Clone, Copy)]
struct Page {
    editor: Editor,
}

impl Component for Page {
    fn body(self, _ctx: &Context) -> impl View {
        custom(self.editor)
    }
}

#[test]
fn the_editor_snippet_compiles_and_reacts() {
    let world = World::new();
    let block = world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(Color::hex(0xC8C8C8)));
    let editor = Editor {
        world,
        camera: State::new(Camera::looking_at(vec3(0.0, 0.0, 6.0), vec3(0.0, 0.0, 0.0))),
        orbit: Orbit::new(),
        tool: State::new(Tool::Push),
        under: State::new(None),
    };
    let page = Page { editor };
    let runtime = Runtime::new();
    let _ = runtime.display_frame(&page, SIZE);
    // hover the front face, then push it toward the eye
    runtime.pointer_moved(200.0, 150.0, bunny_ui::action::Modifiers::NONE);
    assert!(editor.under.get().is_some(), "the pointer is over the block");
    runtime.pointer_pressed(200.0, 150.0);
    runtime.pointer_released(200.0, 150.0);
    assert!((block.position().z - 0.25).abs() < 1e-9, "the push moved the block along the face normal: {:?}", block.position());
    let _ = runtime.display_frame(&page, SIZE);
    let _ = Tool::Select;
}
