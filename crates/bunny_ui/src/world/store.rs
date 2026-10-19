//! The retained world: entities in slots, read and written through one
//! `State`.
//!
//! A [`World`] is a handle, `Copy` like a `State`, and the world it names
//! lives in the state arena. Every change is ONE write, so a box that
//! painted the world repaints once, and no body runs. A change that
//! changes nothing writes nothing.
//!
//! The slots live in chunks of [`CHUNK`] behind `Rc`. A frame that keeps
//! a snapshot of the world keeps the chunks alive; the next edit copies
//! only the chunk it touches. A snapshot is a clone of a short list of
//! `Rc`s, never a copy of the entities.

use std::rc::Rc;

use motor::hash::FxHashMap as HashMap;
use motor::state::State;

use super::material::{FloorMap, Light, LineStyle, Material, Model, PartState};
use super::math::{Aabb, Mat4, Quat, Transform, Vec3};
use super::mesh::{Lines, Mesh};
use crate::layout::Color;

/// Slots per chunk: the unit an edit copies when a snapshot shares it.
pub const CHUNK: usize = 128;

// MARK: - What an entity is

/// What an entity shows.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Shape {
    /// Nothing of its own: a group that moves its children, a pivot.
    #[default]
    Empty,
    Model(Model),
    Lines(Lines, LineStyle),
    Light(Light),
}

/// Everything about an entity but its place in the hierarchy: the value
/// [`World::sync`] compares to know whether an entity changed.
#[derive(Clone, Debug, PartialEq)]
pub struct EntitySpec {
    pub shape: Shape,
    pub transform: Transform,
    pub visible: bool,
    pub pickable: bool,
    pub casts_shadow: bool,
    /// The entity's surfaces read the world's floor map
    /// ([`World::set_floor_map`]): a floor, a rug, a terrace.
    pub reads_floor_map: bool,
    /// The group of lights the entity's surfaces answer to, 0 to 31: a
    /// point light reaches the groups its mask names
    /// ([`super::material::PointLight::groups`]). A lamp in one room
    /// does not light the next.
    pub light_group: u8,
    /// Multiplies the colour of every material of the entity.
    pub tint: Color,
    /// Multiplies the light the entity takes from the sky and the ground:
    /// the ambient of the room it stands in. White by default.
    pub ambient: Color,
    /// The state of each part of the entity's mesh, by part number: a
    /// part takes a tint of its own, or hides, with no new mesh. A part
    /// past the end keeps its own look.
    pub parts: Option<Rc<[PartState]>>,
    /// A number the app chooses, given back with a pick.
    pub tag: u64,
    /// Raw JSON an asset brought with it, untouched.
    pub extras: Option<Rc<str>>,
}

impl Default for EntitySpec {
    fn default() -> EntitySpec {
        EntitySpec::new(Shape::Empty)
    }
}

impl EntitySpec {
    pub fn new(shape: Shape) -> EntitySpec {
        EntitySpec {
            shape,
            transform: Transform::IDENTITY,
            visible: true,
            pickable: true,
            casts_shadow: true,
            reads_floor_map: false,
            light_group: 0,
            tint: Color::WHITE,
            ambient: Color::WHITE,
            parts: None,
            tag: 0,
            extras: None,
        }
    }

    /// A mesh in one material.
    pub fn model(mesh: Mesh, material: Material) -> EntitySpec {
        EntitySpec::new(Shape::Model(Model::single(mesh, material)))
    }

    pub fn lines(lines: Lines, style: LineStyle) -> EntitySpec {
        EntitySpec { casts_shadow: false, ..EntitySpec::new(Shape::Lines(lines, style)) }
    }

    pub fn light(light: impl Into<Light>) -> EntitySpec {
        EntitySpec { pickable: false, casts_shadow: false, ..EntitySpec::new(Shape::Light(light.into())) }
    }

    pub fn at(self, position: Vec3) -> EntitySpec {
        EntitySpec { transform: Transform { translation: position, ..self.transform }, ..self }
    }

    pub fn rotated(self, rotation: Quat) -> EntitySpec {
        EntitySpec { transform: Transform { rotation, ..self.transform }, ..self }
    }

    pub fn scaled(self, scale: Vec3) -> EntitySpec {
        EntitySpec { transform: Transform { scale, ..self.transform }, ..self }
    }

    pub fn tinted(self, tint: Color) -> EntitySpec {
        EntitySpec { tint, ..self }
    }

    pub fn tagged(self, tag: u64) -> EntitySpec {
        EntitySpec { tag, ..self }
    }

    pub fn hidden(self) -> EntitySpec {
        EntitySpec { visible: false, ..self }
    }

    pub fn not_pickable(self) -> EntitySpec {
        EntitySpec { pickable: false, ..self }
    }

    pub fn without_shadow(self) -> EntitySpec {
        EntitySpec { casts_shadow: false, ..self }
    }

    /// The entity's surfaces read the world's floor map.
    pub fn reading_floor_map(self) -> EntitySpec {
        EntitySpec { reads_floor_map: true, ..self }
    }

    /// The entity's surfaces answer to the lights of `group` (0 to 31).
    pub fn in_light_group(self, group: u8) -> EntitySpec {
        EntitySpec { light_group: group.min(31), ..self }
    }

    /// The box around what the entity shows, in its own space. Empty for
    /// a group and a light.
    pub fn local_bounds(&self) -> Aabb {
        match &self.shape {
            Shape::Model(model) => model.mesh.bounds(),
            Shape::Lines(lines, _) => lines.bounds(),
            Shape::Empty | Shape::Light(_) => Aabb::EMPTY,
        }
    }
}

/// The name of an entity inside its world: a slot and the generation of
/// that slot. A removed entity's name never comes back: the slot's next
/// tenant has a new generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntityId {
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EntityData {
    pub(crate) spec: EntitySpec,
    pub(crate) parent: Option<EntityId>,
    /// The key [`World::sync`] knows the entity by.
    pub(crate) key: Option<u64>,
}

#[derive(Clone)]
pub(crate) struct Slot {
    pub(crate) generation: u32,
    pub(crate) entity: Option<EntityData>,
}

#[derive(Clone)]
pub(crate) struct Chunk {
    pub(crate) slots: Vec<Slot>,
}

// MARK: - The data behind the state

/// The world's content. Cloning it clones a list of `Rc`s.
#[derive(Clone)]
pub(crate) struct WorldData {
    pub(crate) chunks: Vec<Rc<Chunk>>,
    free: Rc<Vec<u32>>,
    keyed: Rc<HashMap<u64, EntityId>>,
    live: u32,
    /// The light and shade baked on the ground.
    pub(crate) floor_map: Option<Rc<FloorMap>>,
}

impl WorldData {
    fn new() -> WorldData {
        WorldData { chunks: Vec::new(), free: Rc::default(), keyed: Rc::default(), live: 0, floor_map: None }
    }

    fn slot(&self, id: EntityId) -> Option<&Slot> {
        let index = id.index as usize;
        self.chunks.get(index / CHUNK)?.slots.get(index % CHUNK).filter(|slot| slot.generation == id.generation)
    }

    pub(crate) fn get(&self, id: EntityId) -> Option<&EntityData> {
        self.slot(id)?.entity.as_ref()
    }

    /// The entity, writable. The chunk is copied first if a snapshot
    /// still shares it.
    fn get_mut(&mut self, id: EntityId) -> Option<&mut EntityData> {
        self.get(id)?;
        let index = id.index as usize;
        let chunk = Rc::make_mut(&mut self.chunks[index / CHUNK]);
        chunk.slots[index % CHUNK].entity.as_mut()
    }

    fn insert(&mut self, data: EntityData) -> EntityId {
        self.live += 1;
        if let Some(index) = Rc::make_mut(&mut self.free).pop() {
            let at = index as usize;
            let chunk = Rc::make_mut(&mut self.chunks[at / CHUNK]);
            let slot = &mut chunk.slots[at % CHUNK];
            slot.entity = Some(data);
            return EntityId { index, generation: slot.generation };
        }
        if self.chunks.last().is_none_or(|chunk| chunk.slots.len() == CHUNK) {
            self.chunks.push(Rc::new(Chunk { slots: Vec::with_capacity(CHUNK) }));
        }
        let chunk_index = self.chunks.len() - 1;
        let chunk = Rc::make_mut(&mut self.chunks[chunk_index]);
        chunk.slots.push(Slot { generation: 0, entity: Some(data) });
        EntityId { index: (chunk_index * CHUNK + chunk.slots.len() - 1) as u32, generation: 0 }
    }

    /// Removes the entity and everything below it. `false` if it was
    /// already gone.
    fn remove(&mut self, id: EntityId) -> bool {
        if self.get(id).is_none() {
            return false;
        }
        let mut doomed = vec![id];
        let mut next = 0;
        while next < doomed.len() {
            let parent = doomed[next];
            next += 1;
            doomed.extend(self.iter().filter(|(_, data)| data.parent == Some(parent)).map(|(child, _)| child));
        }
        for id in doomed {
            let index = id.index as usize;
            let chunk = Rc::make_mut(&mut self.chunks[index / CHUNK]);
            let slot = &mut chunk.slots[index % CHUNK];
            if let Some(data) = slot.entity.take() {
                if let Some(key) = data.key {
                    Rc::make_mut(&mut self.keyed).remove(&key);
                }
                slot.generation = slot.generation.wrapping_add(1);
                Rc::make_mut(&mut self.free).push(id.index);
                self.live -= 1;
            }
        }
        true
    }

    /// Every live entity, in slot order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (EntityId, &EntityData)> + '_ {
        self.chunks.iter().enumerate().flat_map(|(chunk_index, chunk)| {
            chunk.slots.iter().enumerate().filter_map(move |(slot_index, slot)| {
                slot.entity.as_ref().map(|data| {
                    (EntityId { index: (chunk_index * CHUNK + slot_index) as u32, generation: slot.generation }, data)
                })
            })
        })
    }

    /// The entity's place in the world: its own transform after each of
    /// its parents'. A parent chain longer than the world is a cycle and
    /// stops there.
    pub(crate) fn world_matrix(&self, id: EntityId) -> Option<Mat4> {
        let mut data = self.get(id)?;
        let mut matrix = data.spec.transform.matrix();
        let mut steps = 0u32;
        while let Some(parent) = data.parent {
            let Some(parent_data) = self.get(parent) else { break };
            matrix = parent_data.spec.transform.matrix() * matrix;
            data = parent_data;
            steps += 1;
            if steps > self.live {
                break;
            }
        }
        Some(matrix)
    }

    /// Is the entity shown — itself and every parent above it?
    pub(crate) fn shown(&self, id: EntityId) -> bool {
        let mut current = Some(id);
        let mut steps = 0u32;
        while let Some(at) = current {
            let Some(data) = self.get(at) else { return false };
            if !data.spec.visible {
                return false;
            }
            current = data.parent;
            steps += 1;
            if steps > self.live + 1 {
                break;
            }
        }
        true
    }

    fn is_below(&self, candidate: EntityId, ancestor: EntityId) -> bool {
        let mut current = Some(candidate);
        let mut steps = 0u32;
        while let Some(at) = current {
            if at == ancestor {
                return true;
            }
            current = self.get(at).and_then(|data| data.parent);
            steps += 1;
            if steps > self.live + 1 {
                return true;
            }
        }
        false
    }

    pub(crate) fn len(&self) -> usize {
        self.live as usize
    }
}

// MARK: - The handles

/// A world of entities. `Copy`; it names a world in the state arena, so
/// a view can keep it in a field the way it keeps a `State`.
///
/// Make it where the app makes its state: `World::new()` inside a body
/// is a declaration, and the same world comes back on every run.
#[derive(Clone, Copy)]
pub struct World {
    data: State<WorldData>,
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World").field("version", &self.version()).finish()
    }
}

impl PartialEq for World {
    fn eq(&self, other: &World) -> bool {
        self.version().0 == other.version().0
    }
}

impl Default for World {
    fn default() -> World {
        World::new()
    }
}

impl World {
    pub fn new() -> World {
        World { data: State::new(WorldData::new()) }
    }

    /// A mesh in one material, at the origin.
    pub fn add(&self, mesh: Mesh, material: Material) -> Entity {
        self.add_spec(EntitySpec::model(mesh, material))
    }

    pub fn add_model(&self, model: Model) -> Entity {
        self.add_spec(EntitySpec::new(Shape::Model(model)))
    }

    pub fn add_lines(&self, lines: Lines, style: LineStyle) -> Entity {
        self.add_spec(EntitySpec::lines(lines, style))
    }

    pub fn add_light(&self, light: impl Into<Light>) -> Entity {
        self.add_spec(EntitySpec::light(light))
    }

    /// An empty entity: a group to move other entities with.
    pub fn add_group(&self) -> Entity {
        self.add_spec(EntitySpec::new(Shape::Empty))
    }

    /// The world's three axes from the origin, `length` meters each way —
    /// x red, y green, z blue, solid where they run positive and dashed
    /// where they run negative: the guide of a modelling tool. ONE group,
    /// in one write: move it, hide it or remove it whole.
    pub fn add_axes(&self, length: f64) -> Entity {
        let reach = length.max(0.0) as f32;
        let axes = [(0usize, Color::rgb(204, 52, 52)), (1, Color::rgb(46, 150, 64)), (2, Color::rgb(52, 92, 210))];
        self.edit(|edit| {
            let group = edit.add(EntitySpec::new(Shape::Empty));
            for (axis, color) in axes {
                for sign in [1.0f32, -1.0] {
                    let mut end = [0.0f32; 3];
                    end[axis] = reach * sign;
                    let style = LineStyle::solid(color, 1.0);
                    let style = if sign < 0.0 { style.dashed(6.0, 4.0) } else { style };
                    let line = edit.add(EntitySpec { pickable: false, ..EntitySpec::lines(Lines::new(vec![[[0.0; 3], end]]), style) });
                    edit.set_parent(line, Some(group));
                }
            }
            group
        })
    }

    pub fn add_spec(&self, spec: EntitySpec) -> Entity {
        let id = self.data.update(|data| data.insert(EntityData { spec, parent: None, key: None }));
        Entity { world: *self, id }
    }

    /// Lays a picture of baked light and shade on the ground, read by the
    /// entities that ask for it; `None` takes it away. One write, and
    /// none when the map is the one already laid.
    pub fn set_floor_map(&self, map: Option<FloorMap>) {
        self.data.update_if(|data| {
            let same = data.floor_map.as_deref() == map.as_ref();
            if !same {
                data.floor_map = map.map(Rc::new);
            }
            (!same, ())
        });
    }

    /// Many changes as ONE write: the box that shows the world repaints
    /// once for all of them.
    pub fn edit<R>(&self, f: impl FnOnce(&mut WorldEdit<'_>) -> R) -> R {
        let world = *self;
        self.data.update_if(|data| {
            let mut edit = WorldEdit { data, world, changed: false };
            let result = f(&mut edit);
            (edit.changed, result)
        })
    }

    /// Makes the keyed part of the world match `items`: an entity per
    /// key, added when the key is new, changed when its spec moved, and
    /// removed when the key is gone. Entities added without a key are
    /// left alone. ONE write when anything changed, and none at all
    /// when nothing did — `true` when it wrote.
    ///
    /// This is the door for a world that is a function of the app's own
    /// data: hand it the whole picture each time, and only the
    /// difference reaches the screen.
    pub fn sync(&self, items: impl IntoIterator<Item = (u64, EntitySpec)>) -> bool {
        let items: Vec<(u64, EntitySpec)> = items.into_iter().collect();
        self.data.update_if(|data| {
            let mut changed = false;
            let mut seen = HashMap::<u64, ()>::default();
            for (key, spec) in items {
                seen.insert(key, ());
                match data.keyed.get(&key).copied().filter(|&id| data.get(id).is_some()) {
                    Some(id) => {
                        let entity = data.get_mut(id).expect("checked alive");
                        if entity.spec != spec {
                            entity.spec = spec;
                            changed = true;
                        }
                    }
                    None => {
                        let id = data.insert(EntityData { spec, parent: None, key: Some(key) });
                        Rc::make_mut(&mut data.keyed).insert(key, id);
                        changed = true;
                    }
                }
            }
            let gone: Vec<EntityId> =
                data.keyed.iter().filter(|(key, _)| !seen.contains_key(key)).map(|(_, &id)| id).collect();
            for id in gone {
                changed |= data.remove(id);
            }
            (changed, changed)
        })
    }

    /// The entity [`World::sync`] keeps for `key`.
    pub fn keyed(&self, key: u64) -> Option<Entity> {
        self.data.with(|data| data.keyed.get(&key).copied().filter(|&id| data.get(id).is_some()))
            .map(|id| Entity { world: *self, id })
    }

    /// How many entities live in the world. Reads the world.
    pub fn len(&self) -> usize {
        self.data.with(WorldData::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The box around everything the world shows. Reads the world.
    pub fn bounds(&self) -> Aabb {
        self.data.with(|data| {
            data.iter()
                .filter(|(id, _)| data.shown(*id))
                .fold(Aabb::EMPTY, |bounds, (id, entity)| {
                    let matrix = data.world_matrix(id).unwrap_or_default();
                    bounds.union(entity.spec.local_bounds().transformed(&matrix))
                })
        })
    }

    /// The world's identity and the writes it has taken. Equal versions
    /// are equal content. Records no read: asking whether something
    /// changed is not reading it.
    pub fn version(&self) -> (u64, u64) {
        self.data.version()
    }

    /// Reads the whole world in place. Records a read.
    pub(crate) fn with<R>(&self, f: impl FnOnce(&WorldData) -> R) -> R {
        self.data.with(f)
    }
}

/// Many changes to one world, applied as one write by [`World::edit`].
pub struct WorldEdit<'a> {
    data: &'a mut WorldData,
    world: World,
    changed: bool,
}

impl WorldEdit<'_> {
    pub fn add(&mut self, spec: EntitySpec) -> Entity {
        self.changed = true;
        let id = self.data.insert(EntityData { spec, parent: None, key: None });
        Entity { world: self.world, id }
    }

    /// The entity's spec as it stands inside this edit.
    pub fn spec(&self, entity: Entity) -> Option<&EntitySpec> {
        self.data.get(entity.id).map(|data| &data.spec)
    }

    /// Changes the entity's spec in place. A change that changes nothing
    /// is not a write.
    pub fn update(&mut self, entity: Entity, f: impl FnOnce(&mut EntitySpec)) {
        let Some(current) = self.data.get(entity.id) else { return };
        let mut spec = current.spec.clone();
        f(&mut spec);
        if spec != current.spec {
            self.data.get_mut(entity.id).expect("checked alive").spec = spec;
            self.changed = true;
        }
    }

    pub fn set_position(&mut self, entity: Entity, position: Vec3) {
        self.update(entity, |spec| spec.transform.translation = position);
    }

    pub fn set_transform(&mut self, entity: Entity, transform: Transform) {
        self.update(entity, |spec| spec.transform = transform);
    }

    pub fn show(&mut self, entity: Entity, shown: bool) {
        self.update(entity, |spec| spec.visible = shown);
    }

    /// Hangs `child` under `parent` (or frees it with `None`). Refused —
    /// `false` — when `parent` already hangs below `child`.
    pub fn set_parent(&mut self, child: Entity, parent: Option<Entity>) -> bool {
        let parent_id = parent.map(|parent| parent.id);
        if let Some(parent_id) = parent_id
            && (self.data.get(parent_id).is_none() || self.data.is_below(parent_id, child.id))
        {
            return false;
        }
        let Some(current) = self.data.get(child.id) else { return false };
        if current.parent != parent_id {
            self.data.get_mut(child.id).expect("checked alive").parent = parent_id;
            self.changed = true;
        }
        true
    }

    /// Removes the entity and everything below it.
    pub fn remove(&mut self, entity: Entity) {
        self.changed |= self.data.remove(entity.id);
    }
}

/// One entity of a world. `Copy`: a view keeps it, a closure captures
/// it. Each setter is one write and returns the entity, so a line can
/// add and place: `world.add(mesh, material).set_position(p)`.
///
/// A removed entity stays removed: its setters do nothing and its
/// readers answer `None` or the default.
#[derive(Clone, Copy, PartialEq)]
pub struct Entity {
    world: World,
    id: EntityId,
}

impl std::fmt::Debug for Entity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entity").field("index", &self.id.index).field("generation", &self.id.generation).finish()
    }
}

impl Entity {
    pub(crate) fn from_parts(world: World, id: EntityId) -> Entity {
        Entity { world, id }
    }

    pub fn id(&self) -> EntityId {
        self.id
    }

    pub fn world(&self) -> World {
        self.world
    }

    /// Writes only when `f` changed the spec.
    fn change(self, f: impl FnOnce(&mut EntitySpec)) -> Entity {
        self.world.edit(|edit| edit.update(self, f));
        self
    }

    pub fn is_alive(&self) -> bool {
        self.world.with(|data| data.get(self.id).is_some())
    }

    /// A copy of the entity's spec. Reads the world.
    pub fn spec(&self) -> Option<EntitySpec> {
        self.world.with(|data| data.get(self.id).map(|entity| entity.spec.clone()))
    }

    pub fn position(&self) -> Vec3 {
        self.transform().translation
    }

    pub fn transform(&self) -> Transform {
        self.world.with(|data| data.get(self.id).map(|entity| entity.spec.transform)).unwrap_or_default()
    }

    /// Where the entity stands in the world, after its parents.
    pub fn world_matrix(&self) -> Mat4 {
        self.world.with(|data| data.world_matrix(self.id)).unwrap_or_default()
    }

    /// The box around what the entity shows, in the world.
    pub fn bounds(&self) -> Aabb {
        self.world.with(|data| {
            let Some(entity) = data.get(self.id) else { return Aabb::EMPTY };
            entity.spec.local_bounds().transformed(&data.world_matrix(self.id).unwrap_or_default())
        })
    }

    pub fn tag(&self) -> u64 {
        self.world.with(|data| data.get(self.id).map_or(0, |entity| entity.spec.tag))
    }

    pub fn parent(&self) -> Option<Entity> {
        self.world.with(|data| data.get(self.id).and_then(|entity| entity.parent))
            .map(|id| Entity { world: self.world, id })
    }

    pub fn set_position(self, position: Vec3) -> Entity {
        self.change(|spec| spec.transform.translation = position)
    }

    pub fn set_rotation(self, rotation: Quat) -> Entity {
        self.change(|spec| spec.transform.rotation = rotation)
    }

    pub fn set_scale(self, scale: Vec3) -> Entity {
        self.change(|spec| spec.transform.scale = scale)
    }

    pub fn set_transform(self, transform: Transform) -> Entity {
        self.change(|spec| spec.transform = transform)
    }

    pub fn set_shape(self, shape: Shape) -> Entity {
        self.change(|spec| spec.shape = shape)
    }

    pub fn show(self, shown: bool) -> Entity {
        self.change(|spec| spec.visible = shown)
    }

    pub fn set_pickable(self, pickable: bool) -> Entity {
        self.change(|spec| spec.pickable = pickable)
    }

    pub fn set_casts_shadow(self, casts: bool) -> Entity {
        self.change(|spec| spec.casts_shadow = casts)
    }

    /// Whether the entity's surfaces read the world's floor map.
    pub fn set_reads_floor_map(self, reads: bool) -> Entity {
        self.change(|spec| spec.reads_floor_map = reads)
    }

    /// The entity's surfaces answer to the lights of `group` (0 to 31).
    pub fn set_light_group(self, group: u8) -> Entity {
        self.change(|spec| spec.light_group = group.min(31))
    }

    pub fn set_tint(self, tint: Color) -> Entity {
        self.change(|spec| spec.tint = tint)
    }

    /// Multiplies the light the entity takes from the sky and the ground.
    pub fn set_ambient(self, ambient: Color) -> Entity {
        self.change(|spec| spec.ambient = ambient)
    }

    /// The state of each part of the entity's mesh, by part number;
    /// `None` gives every part its own look back.
    pub fn set_parts(self, parts: Option<Rc<[PartState]>>) -> Entity {
        self.change(|spec| spec.parts = parts)
    }

    pub fn set_tag(self, tag: u64) -> Entity {
        self.change(|spec| spec.tag = tag)
    }

    /// Hangs the entity under `parent`: it moves, turns and hides with
    /// it. `None` frees it. A parent that hangs below this entity is
    /// refused.
    pub fn set_parent(self, parent: Option<Entity>) -> Entity {
        self.world.edit(|edit| edit.set_parent(self, parent));
        self
    }

    /// Removes the entity and everything that hangs below it.
    pub fn remove(self) {
        self.world.edit(|edit| edit.remove(self));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::math::vec3;

    fn cube() -> Mesh {
        Mesh::cuboid(1.0, 1.0, 1.0)
    }

    fn red() -> Material {
        Material::color(Color::rgb(200, 40, 40))
    }

    #[test]
    fn an_old_snapshot_does_not_see_a_new_edit() {
        let world = World::new();
        let entity = world.add(cube(), red());
        let before = world.with(WorldData::clone);
        entity.set_position(vec3(5.0, 0.0, 0.0));
        let old = before.get(entity.id()).expect("the snapshot kept the entity");
        assert_eq!(old.spec.transform.translation, Vec3::ZERO, "the snapshot is frozen");
        assert_eq!(entity.position(), vec3(5.0, 0.0, 0.0));
    }

    #[test]
    fn an_edit_copies_only_the_chunks_it_touched() {
        let world = World::new();
        let entities: Vec<Entity> = (0..CHUNK * 3).map(|_| world.add(cube(), red())).collect();
        let before = world.with(WorldData::clone);
        entities[CHUNK + 5].set_position(vec3(1.0, 2.0, 3.0));
        let after = world.with(WorldData::clone);
        assert_eq!(before.chunks.len(), 3);
        assert!(Rc::ptr_eq(&before.chunks[0], &after.chunks[0]), "an untouched chunk is shared");
        assert!(!Rc::ptr_eq(&before.chunks[1], &after.chunks[1]), "the touched chunk was copied");
        assert!(Rc::ptr_eq(&before.chunks[2], &after.chunks[2]));
    }

    #[test]
    fn a_removed_entity_is_stale_for_good() {
        let world = World::new();
        let first = world.add(cube(), red());
        first.remove();
        assert!(!first.is_alive());
        // the slot comes back with a new generation
        let second = world.add(cube(), red());
        assert_eq!(second.id().index, first.id().index, "the slot is reused");
        assert!(!first.is_alive(), "the old name does not reach the new tenant");
        let version = world.version();
        first.set_position(vec3(9.0, 9.0, 9.0));
        assert_eq!(world.version(), version, "a setter on a removed entity writes nothing");
        assert_eq!(second.position(), Vec3::ZERO);
    }

    #[test]
    fn a_change_that_changes_nothing_writes_nothing() {
        let world = World::new();
        let entity = world.add(cube(), red()).set_position(vec3(1.0, 0.0, 0.0));
        let version = world.version();
        entity.set_position(vec3(1.0, 0.0, 0.0));
        assert_eq!(world.version(), version);
        entity.set_position(vec3(2.0, 0.0, 0.0));
        assert_ne!(world.version(), version);
    }

    #[test]
    fn one_edit_is_one_write() {
        let world = World::new();
        let (_, writes) = world.version();
        world.edit(|edit| {
            for i in 0..10 {
                edit.add(EntitySpec::model(cube(), red()).at(vec3(i as f64, 0.0, 0.0)));
            }
        });
        assert_eq!(world.version().1, writes + 1);
        assert_eq!(world.len(), 10);
    }

    #[test]
    fn sync_writes_nothing_when_nothing_changed() {
        let world = World::new();
        let mesh = cube();
        let picture = |count: u64, x: f64| -> Vec<(u64, EntitySpec)> {
            (0..count).map(|key| (key, EntitySpec::model(mesh.clone(), red()).at(vec3(key as f64 * x, 0.0, 0.0)))).collect()
        };
        assert!(world.sync(picture(5, 1.0)));
        assert_eq!(world.len(), 5);
        let version = world.version();
        assert!(!world.sync(picture(5, 1.0)), "the same picture again");
        assert_eq!(world.version(), version, "and not a single write");

        assert!(world.sync(picture(3, 2.0)), "two keys gone, the rest moved");
        assert_eq!(world.len(), 3);
        assert_eq!(world.keyed(2).expect("key 2 stays").position(), vec3(4.0, 0.0, 0.0));
        assert!(world.keyed(4).is_none(), "key 4 left");
        // an entity added by hand is not the sync's business
        let loose = world.add(cube(), red());
        assert!(world.sync(Vec::new()));
        assert!(loose.is_alive());
        assert_eq!(world.len(), 1);
    }

    #[test]
    fn a_child_follows_its_parent() {
        let world = World::new();
        let parent = world.add_group().set_position(vec3(10.0, 0.0, 0.0));
        let child = world.add(cube(), red()).set_position(vec3(1.0, 0.0, 0.0)).set_parent(Some(parent));
        let at = child.world_matrix().transform_point(Vec3::ZERO);
        assert_eq!(at, vec3(11.0, 0.0, 0.0));
        parent.set_rotation(Quat::from_yaw(std::f64::consts::FRAC_PI_2));
        let turned = child.world_matrix().transform_point(Vec3::ZERO);
        assert!((turned - vec3(10.0, 0.0, -1.0)).length() < 1e-12, "{turned:?}");
        // the box follows too
        assert!(child.bounds().contains(vec3(10.0, 0.0, -1.0)));
        // a cycle is refused
        parent.set_parent(Some(child));
        assert_eq!(parent.parent(), None);
        // hiding the parent hides the child; removing it removes the child
        parent.show(false);
        assert!(!world.with(|data| data.shown(child.id())));
        parent.remove();
        assert!(!child.is_alive());
        assert!(world.is_empty());
    }

    #[test]
    fn the_world_bounds_hold_what_it_shows() {
        let world = World::new();
        world.add(cube(), red()).set_position(vec3(-3.0, 0.0, 0.0));
        world.add(cube(), red()).set_position(vec3(4.0, 2.0, 0.0));
        world.add_light(Light::point(Color::WHITE)).set_position(vec3(100.0, 0.0, 0.0));
        let hidden = world.add(cube(), red()).set_position(vec3(0.0, -50.0, 0.0));
        hidden.show(false);
        let bounds = world.bounds();
        assert_eq!(bounds.min, vec3(-3.5, -0.5, -0.5), "a light and a hidden entity add nothing");
        assert_eq!(bounds.max, vec3(4.5, 2.5, 0.5));
    }
}
