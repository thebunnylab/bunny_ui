//! Vulkan presentation — the front of the ladder, by the user's own
//! decision: vk first, gl when it cannot come up, cpu last.
//!
//! House rules apply: no dependencies. The loader comes in through
//! `dlopen` of the system's `libvulkan.so.1`; every entry point
//! resolves through `vkGetInstanceProcAddr`. The shaders are SPIR-V
//! blobs COMMITTED beside their GLSL sources under `src/shaders/` —
//! Vulkan ships no runtime compiler in the OS, so the repo carries
//! our own baked artifacts and a gated test recompiles-and-compares
//! whenever `glslangValidator` is on the box (absent compiler = an
//! honest skip; the build itself never needs one).
//!
//! The platform-neutral half — wire structs, shelf atlas, batching,
//! the walk — is `walk.rs`, shared verbatim with the gl tier. This
//! module only owns what Vulkan makes different:
//! - the swapchain (IMMEDIATE → MAILBOX → FIFO; the first two are the
//!   SwapInterval(0) twin — pacing stays with the shell's clocks);
//! - push constants carry the viewport, the per-run round clip and
//!   the mask quad in one 48-byte range — no UBO machinery at all;
//! - per-run base = the vertex-buffer bind OFFSET (the same road the
//!   gl tier walks; no instance-index arithmetic anywhere);
//! - three frames in flight, each slot owning its instance buffers,
//!   its staging arena, its command buffer and its fence;
//! - and one mercy: Vulkan's NDC and fragcoord already point DOWN —
//!   the raster's own direction, so the gl tier's two flips simply
//!   do not exist here.
//!
//! WSI speaks both doors natively (`wl_surface` and xcb window) —
//! no `wl_egl_window` anywhere on this tier. A lost device rebuilds
//! once in silence, then hands the window to the gl tier for life.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};

use bunny_ui::image_engine::ImageEngine;
use bunny_ui::layout::{Color, DisplayList, Size};
use bunny_ui::text_engine::TextEngine;

use crate::walk::{
    build_frame, AtlasFull, AtlasGround, FrameBatches, RectInstance, RoundClip, RunAtlas,
    RunKind, SpriteInstance,
};

// MARK: - The committed shaders (SPIR-V beside their GLSL truth)

const RECT_VERT_SPV: &[u8] = include_bytes!("shaders/rect.vert.spv");
const RECT_FRAG_SPV: &[u8] = include_bytes!("shaders/rect.frag.spv");
const SPRITE_VERT_SPV: &[u8] = include_bytes!("shaders/sprite.vert.spv");
const SPRITE_FRAG_SPV: &[u8] = include_bytes!("shaders/sprite.frag.spv");
const MASK_VERT_SPV: &[u8] = include_bytes!("shaders/mask.vert.spv");
const MASK_FRAG_SPV: &[u8] = include_bytes!("shaders/mask.frag.spv");

/// The 56-byte push range every pipeline shares — layout mirrored in
/// each shader's `Push` block, asserts are the defense against drift.
#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    round_box: [f32; 4],
    quad: [f32; 4],
    /// The cut's four corners. A `vec4` wants a 16-byte slot, so it
    /// sits before the viewport and the range grows by eight bytes —
    /// far under the 128 every device promises.
    round_radii: [f32; 4],
    viewport: [f32; 2],
}

const _: () = {
    assert!(std::mem::size_of::<Push>() == 56);
    assert!(std::mem::offset_of!(Push, quad) == 16);
    assert!(std::mem::offset_of!(Push, round_radii) == 32);
    assert!(std::mem::offset_of!(Push, viewport) == 48);
};

// MARK: - FFI border (dlopen the door, GetInstanceProcAddr the hallway)

unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

const RTLD_NOW: i32 = 2;

// Handles are opaque 64-bit values (dispatchable ones are pointers;
// the distinction never matters to the caller).
type Instance = *mut c_void;
type PhysicalDevice = *mut c_void;
type Device = *mut c_void;
type Queue = *mut c_void;
type CommandBuffer = *mut c_void;
type SurfaceKHR = u64;
type SwapchainKHR = u64;
type ImageHandle = u64;
type ImageView = u64;
type ShaderModule = u64;
type PipelineLayout = u64;
type RenderPass = u64;
type Pipeline = u64;
type Framebuffer = u64;
type BufferHandle = u64;
type DeviceMemory = u64;
type Fence = u64;
type Semaphore = u64;
type CommandPool = u64;
type Sampler = u64;
type DescriptorSetLayout = u64;
type DescriptorPool = u64;
type DescriptorSet = u64;

type VkResult = i32;
const VK_SUCCESS: VkResult = 0;
const VK_SUBOPTIMAL_KHR: VkResult = 1000001003;
const VK_ERROR_OUT_OF_DATE_KHR: VkResult = -1000001004;
const VK_ERROR_DEVICE_LOST: VkResult = -4;
const VK_ERROR_SURFACE_LOST_KHR: VkResult = -1000000000;

// structure types (vulkan_core.h)
const ST_APPLICATION_INFO: u32 = 0;
const ST_INSTANCE_CREATE: u32 = 1;
const ST_DEVICE_QUEUE_CREATE: u32 = 2;
const ST_DEVICE_CREATE: u32 = 3;
const ST_SUBMIT_INFO: u32 = 4;
const ST_MEMORY_ALLOCATE: u32 = 5;
const ST_FENCE_CREATE: u32 = 8;
const ST_SEMAPHORE_CREATE: u32 = 9;
const ST_BUFFER_CREATE: u32 = 12;
const ST_IMAGE_CREATE: u32 = 14;
const ST_IMAGE_VIEW_CREATE: u32 = 15;
const ST_SHADER_MODULE_CREATE: u32 = 16;
const ST_PIPELINE_SHADER_STAGE: u32 = 18;
const ST_PIPELINE_VERTEX_INPUT: u32 = 19;
const ST_PIPELINE_INPUT_ASSEMBLY: u32 = 20;
const ST_PIPELINE_VIEWPORT: u32 = 22;
const ST_PIPELINE_RASTERIZATION: u32 = 23;
const ST_PIPELINE_MULTISAMPLE: u32 = 24;
const ST_PIPELINE_COLOR_BLEND: u32 = 26;
const ST_PIPELINE_DYNAMIC: u32 = 27;
const ST_GRAPHICS_PIPELINE_CREATE: u32 = 28;
const ST_PIPELINE_LAYOUT_CREATE: u32 = 30;
const ST_SAMPLER_CREATE: u32 = 31;
const ST_DESCRIPTOR_SET_LAYOUT_CREATE: u32 = 32;
const ST_DESCRIPTOR_POOL_CREATE: u32 = 33;
const ST_DESCRIPTOR_SET_ALLOCATE: u32 = 34;
const ST_WRITE_DESCRIPTOR_SET: u32 = 35;
const ST_FRAMEBUFFER_CREATE: u32 = 37;
const ST_RENDER_PASS_CREATE: u32 = 38;
const ST_COMMAND_POOL_CREATE: u32 = 39;
const ST_COMMAND_BUFFER_ALLOCATE: u32 = 40;
const ST_COMMAND_BUFFER_BEGIN: u32 = 42;
const ST_RENDER_PASS_BEGIN: u32 = 43;
const ST_IMAGE_MEMORY_BARRIER: u32 = 45;
const ST_PRESENT_INFO_KHR: u32 = 1000001001;
const ST_SWAPCHAIN_CREATE_KHR: u32 = 1000001000;
const ST_XCB_SURFACE_CREATE_KHR: u32 = 1000005000;
const ST_WAYLAND_SURFACE_CREATE_KHR: u32 = 1000006000;

const FORMAT_B8G8R8A8_UNORM: u32 = 44;
const FORMAT_R8G8B8A8_UNORM: u32 = 37;
const FORMAT_R32G32_SFLOAT: u32 = 103;
const FORMAT_R32G32B32A32_SFLOAT: u32 = 109;

const COLORSPACE_SRGB_NONLINEAR: u32 = 0;
const PRESENT_MODE_IMMEDIATE: u32 = 0;
const PRESENT_MODE_MAILBOX: u32 = 1;
const PRESENT_MODE_FIFO: u32 = 2;

const IMAGE_USAGE_TRANSFER_SRC: u32 = 0x1;
const IMAGE_USAGE_TRANSFER_DST: u32 = 0x2;
const IMAGE_USAGE_SAMPLED: u32 = 0x4;
const IMAGE_USAGE_COLOR_ATTACHMENT: u32 = 0x10;

const BUFFER_USAGE_TRANSFER_SRC: u32 = 0x1;
const BUFFER_USAGE_TRANSFER_DST: u32 = 0x2;
const BUFFER_USAGE_VERTEX: u32 = 0x80;

const MEMORY_PROPERTY_DEVICE_LOCAL: u32 = 0x1;
const MEMORY_PROPERTY_HOST_VISIBLE: u32 = 0x2;
const MEMORY_PROPERTY_HOST_COHERENT: u32 = 0x4;

const IMAGE_LAYOUT_UNDEFINED: u32 = 0;
const IMAGE_LAYOUT_COLOR_ATTACHMENT: u32 = 2;
const IMAGE_LAYOUT_SHADER_READ_ONLY: u32 = 5;
const IMAGE_LAYOUT_TRANSFER_SRC: u32 = 6;
const IMAGE_LAYOUT_TRANSFER_DST: u32 = 7;
const IMAGE_LAYOUT_PRESENT_SRC_KHR: u32 = 1000001002;

const PIPELINE_STAGE_TOP: u32 = 0x1;
const PIPELINE_STAGE_TRANSFER: u32 = 0x1000;
const PIPELINE_STAGE_FRAGMENT_SHADER: u32 = 0x80;
const PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT: u32 = 0x400;

const ACCESS_TRANSFER_WRITE: u32 = 0x1000;
const ACCESS_SHADER_READ: u32 = 0x20;

const SHADER_STAGE_VERTEX: u32 = 0x1;
const SHADER_STAGE_FRAGMENT: u32 = 0x10;

const BLEND_FACTOR_ZERO: u32 = 0;
const BLEND_FACTOR_ONE: u32 = 1;
const BLEND_FACTOR_SRC_ALPHA: u32 = 6;
const BLEND_FACTOR_ONE_MINUS_SRC_ALPHA: u32 = 7;
const BLEND_OP_ADD: u32 = 0;

const VERTEX_INPUT_RATE_INSTANCE: u32 = 1;
const PRIMITIVE_TOPOLOGY_TRIANGLE_LIST: u32 = 3;
const POLYGON_MODE_FILL: u32 = 0;
const CULL_MODE_NONE: u32 = 0;
const FRONT_FACE_CCW: u32 = 1;
const SAMPLE_COUNT_1: u32 = 1;
const DYNAMIC_STATE_VIEWPORT: u32 = 0;
const DYNAMIC_STATE_SCISSOR: u32 = 1;
const ATTACHMENT_LOAD_CLEAR: u32 = 1;
const ATTACHMENT_STORE_STORE: u32 = 0;
const ATTACHMENT_LOAD_DONT_CARE: u32 = 2;
const PIPELINE_BIND_POINT_GRAPHICS: u32 = 0;
const SUBPASS_CONTENTS_INLINE: u32 = 0;
const COMMAND_BUFFER_LEVEL_PRIMARY: u32 = 0;
const COMMAND_POOL_CREATE_RESET_BUFFER: u32 = 0x2;
const COMMAND_BUFFER_USAGE_ONE_TIME: u32 = 0x1;
const FENCE_CREATE_SIGNALED: u32 = 0x1;
const FILTER_NEAREST: u32 = 0;
const SAMPLER_ADDRESS_CLAMP_TO_EDGE: u32 = 2;
const DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER: u32 = 1;
const IMAGE_VIEW_TYPE_2D: u32 = 1;
const IMAGE_TYPE_2D: u32 = 1;
const IMAGE_TILING_OPTIMAL: u32 = 0;
const SHARING_MODE_EXCLUSIVE: u32 = 0;
const IMAGE_ASPECT_COLOR: u32 = 0x1;
const COMPOSITE_ALPHA_OPAQUE: u32 = 0x1;
const COMPOSITE_ALPHA_PREMULTIPLIED: u32 = 0x4;
const COMPOSITE_ALPHA_INHERIT: u32 = 0x8;
const QUEUE_GRAPHICS_BIT: u32 = 0x1;

// MARK: - The structs the API takes (header order, header names)

#[repr(C)]
struct ApplicationInfo {
    s_type: u32,
    p_next: *const c_void,
    app_name: *const c_char,
    app_version: u32,
    engine_name: *const c_char,
    engine_version: u32,
    api_version: u32,
}

#[repr(C)]
struct InstanceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    app_info: *const ApplicationInfo,
    layer_count: u32,
    layers: *const *const c_char,
    extension_count: u32,
    extensions: *const *const c_char,
}

#[repr(C)]
struct DeviceQueueCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    family: u32,
    count: u32,
    priorities: *const f32,
}

#[repr(C)]
struct DeviceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    queue_count: u32,
    queues: *const DeviceQueueCreateInfo,
    layer_count: u32,
    layers: *const *const c_char,
    extension_count: u32,
    extensions: *const *const c_char,
    features: *const c_void,
}

#[repr(C)]
struct XcbSurfaceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    connection: *mut c_void,
    window: u32,
}

#[repr(C)]
struct WaylandSurfaceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    display: *mut c_void,
    surface: *mut c_void,
}

#[repr(C)]
struct SurfaceCapabilities {
    min_image_count: u32,
    max_image_count: u32,
    current_extent: [u32; 2],
    min_extent: [u32; 2],
    max_extent: [u32; 2],
    max_array_layers: u32,
    supported_transforms: u32,
    current_transform: u32,
    supported_composite_alpha: u32,
    supported_usage: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SurfaceFormat {
    format: u32,
    color_space: u32,
}

#[repr(C)]
struct SwapchainCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    surface: SurfaceKHR,
    min_image_count: u32,
    format: u32,
    color_space: u32,
    extent: [u32; 2],
    array_layers: u32,
    usage: u32,
    sharing: u32,
    family_count: u32,
    families: *const u32,
    pre_transform: u32,
    composite_alpha: u32,
    present_mode: u32,
    clipped: u32,
    old_swapchain: SwapchainKHR,
}

#[repr(C)]
struct ImageViewCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    image: ImageHandle,
    view_type: u32,
    format: u32,
    components: [u32; 4],
    aspect: u32,
    base_mip: u32,
    mip_count: u32,
    base_layer: u32,
    layer_count: u32,
}

#[repr(C)]
struct ShaderModuleCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    code_size: usize,
    code: *const u32,
}

#[repr(C)]
struct PushConstantRange {
    stages: u32,
    offset: u32,
    size: u32,
}

#[repr(C)]
struct PipelineLayoutCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    set_layout_count: u32,
    set_layouts: *const DescriptorSetLayout,
    push_range_count: u32,
    push_ranges: *const PushConstantRange,
}

#[repr(C)]
struct AttachmentDescription {
    flags: u32,
    format: u32,
    samples: u32,
    load_op: u32,
    store_op: u32,
    stencil_load: u32,
    stencil_store: u32,
    initial_layout: u32,
    final_layout: u32,
}

#[repr(C)]
struct AttachmentReference {
    attachment: u32,
    layout: u32,
}

#[repr(C)]
struct SubpassDescription {
    flags: u32,
    bind_point: u32,
    input_count: u32,
    inputs: *const AttachmentReference,
    color_count: u32,
    colors: *const AttachmentReference,
    resolves: *const AttachmentReference,
    depth: *const AttachmentReference,
    preserve_count: u32,
    preserves: *const u32,
}

#[repr(C)]
struct RenderPassCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    attachment_count: u32,
    attachments: *const AttachmentDescription,
    subpass_count: u32,
    subpasses: *const SubpassDescription,
    dependency_count: u32,
    dependencies: *const c_void,
}

#[repr(C)]
struct PipelineShaderStage {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    stage: u32,
    module: ShaderModule,
    name: *const c_char,
    specialization: *const c_void,
}

#[repr(C)]
struct VertexInputBinding {
    binding: u32,
    stride: u32,
    input_rate: u32,
}

#[repr(C)]
struct VertexInputAttribute {
    location: u32,
    binding: u32,
    format: u32,
    offset: u32,
}

#[repr(C)]
struct PipelineVertexInput {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    binding_count: u32,
    bindings: *const VertexInputBinding,
    attribute_count: u32,
    attributes: *const VertexInputAttribute,
}

#[repr(C)]
struct PipelineInputAssembly {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    topology: u32,
    primitive_restart: u32,
}

#[repr(C)]
struct PipelineViewportState {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    viewport_count: u32,
    viewports: *const c_void,
    scissor_count: u32,
    scissors: *const c_void,
}

#[repr(C)]
struct PipelineRasterization {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    depth_clamp: u32,
    discard: u32,
    polygon_mode: u32,
    cull_mode: u32,
    front_face: u32,
    depth_bias: u32,
    bias_constant: f32,
    bias_clamp: f32,
    bias_slope: f32,
    line_width: f32,
}

#[repr(C)]
struct PipelineMultisample {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    samples: u32,
    sample_shading: u32,
    min_sample_shading: f32,
    sample_mask: *const u32,
    alpha_to_coverage: u32,
    alpha_to_one: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ColorBlendAttachment {
    blend_enable: u32,
    src_color: u32,
    dst_color: u32,
    color_op: u32,
    src_alpha: u32,
    dst_alpha: u32,
    alpha_op: u32,
    write_mask: u32,
}

#[repr(C)]
struct PipelineColorBlend {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    logic_op_enable: u32,
    logic_op: u32,
    attachment_count: u32,
    attachments: *const ColorBlendAttachment,
    blend_constants: [f32; 4],
}

#[repr(C)]
struct PipelineDynamicState {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    state_count: u32,
    states: *const u32,
}

#[repr(C)]
struct GraphicsPipelineCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    stage_count: u32,
    stages: *const PipelineShaderStage,
    vertex_input: *const PipelineVertexInput,
    input_assembly: *const PipelineInputAssembly,
    tessellation: *const c_void,
    viewport: *const PipelineViewportState,
    rasterization: *const PipelineRasterization,
    multisample: *const PipelineMultisample,
    depth_stencil: *const c_void,
    color_blend: *const PipelineColorBlend,
    dynamic: *const PipelineDynamicState,
    layout: PipelineLayout,
    render_pass: RenderPass,
    subpass: u32,
    base_pipeline: Pipeline,
    base_index: i32,
}

#[repr(C)]
struct FramebufferCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    render_pass: RenderPass,
    attachment_count: u32,
    attachments: *const ImageView,
    width: u32,
    height: u32,
    layers: u32,
}

#[repr(C)]
struct BufferCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    size: u64,
    usage: u32,
    sharing: u32,
    family_count: u32,
    families: *const u32,
}

#[repr(C)]
struct MemoryRequirements {
    size: u64,
    alignment: u64,
    memory_type_bits: u32,
}

#[repr(C)]
struct MemoryAllocateInfo {
    s_type: u32,
    p_next: *const c_void,
    size: u64,
    memory_type: u32,
}

#[repr(C)]
struct MemoryType {
    property_flags: u32,
    heap_index: u32,
}

#[repr(C)]
struct PhysicalDeviceMemoryProperties {
    type_count: u32,
    types: [MemoryType; 32],
    heap_count: u32,
    heaps: [[u64; 2]; 16],
}

#[repr(C)]
struct ImageCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    image_type: u32,
    format: u32,
    extent: [u32; 3],
    mip_levels: u32,
    array_layers: u32,
    samples: u32,
    tiling: u32,
    usage: u32,
    sharing: u32,
    family_count: u32,
    families: *const u32,
    initial_layout: u32,
}

#[repr(C)]
struct SamplerCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    mag_filter: u32,
    min_filter: u32,
    mipmap_mode: u32,
    address_u: u32,
    address_v: u32,
    address_w: u32,
    mip_lod_bias: f32,
    anisotropy_enable: u32,
    max_anisotropy: f32,
    compare_enable: u32,
    compare_op: u32,
    min_lod: f32,
    max_lod: f32,
    border_color: u32,
    unnormalized: u32,
}

#[repr(C)]
struct DescriptorSetLayoutBinding {
    binding: u32,
    descriptor_type: u32,
    count: u32,
    stages: u32,
    immutable_samplers: *const Sampler,
}

#[repr(C)]
struct DescriptorSetLayoutCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    binding_count: u32,
    bindings: *const DescriptorSetLayoutBinding,
}

#[repr(C)]
struct DescriptorPoolSize {
    descriptor_type: u32,
    count: u32,
}

#[repr(C)]
struct DescriptorPoolCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    max_sets: u32,
    pool_size_count: u32,
    pool_sizes: *const DescriptorPoolSize,
}

#[repr(C)]
struct DescriptorSetAllocateInfo {
    s_type: u32,
    p_next: *const c_void,
    pool: DescriptorPool,
    count: u32,
    layouts: *const DescriptorSetLayout,
}

#[repr(C)]
struct DescriptorImageInfo {
    sampler: Sampler,
    image_view: ImageView,
    layout: u32,
}

#[repr(C)]
struct WriteDescriptorSet {
    s_type: u32,
    p_next: *const c_void,
    set: DescriptorSet,
    binding: u32,
    array_element: u32,
    count: u32,
    descriptor_type: u32,
    image_info: *const DescriptorImageInfo,
    buffer_info: *const c_void,
    texel_view: *const c_void,
}

#[repr(C)]
struct CommandPoolCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    family: u32,
}

#[repr(C)]
struct CommandBufferAllocateInfo {
    s_type: u32,
    p_next: *const c_void,
    pool: CommandPool,
    level: u32,
    count: u32,
}

#[repr(C)]
struct CommandBufferBeginInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    inheritance: *const c_void,
}

#[repr(C)]
struct FenceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
}

#[repr(C)]
struct SemaphoreCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
}

#[repr(C)]
struct SubmitInfo {
    s_type: u32,
    p_next: *const c_void,
    wait_count: u32,
    wait_semaphores: *const Semaphore,
    wait_stages: *const u32,
    command_buffer_count: u32,
    command_buffers: *const CommandBuffer,
    signal_count: u32,
    signal_semaphores: *const Semaphore,
}

#[repr(C)]
struct PresentInfo {
    s_type: u32,
    p_next: *const c_void,
    wait_count: u32,
    wait_semaphores: *const Semaphore,
    swapchain_count: u32,
    swapchains: *const SwapchainKHR,
    image_indices: *const u32,
    results: *mut VkResult,
}

#[repr(C)]
struct ImageMemoryBarrier {
    s_type: u32,
    p_next: *const c_void,
    src_access: u32,
    dst_access: u32,
    old_layout: u32,
    new_layout: u32,
    src_family: u32,
    dst_family: u32,
    image: ImageHandle,
    aspect: u32,
    base_mip: u32,
    mip_count: u32,
    base_layer: u32,
    layer_count: u32,
}

#[repr(C)]
struct BufferImageCopy {
    buffer_offset: u64,
    buffer_row_length: u32,
    buffer_image_height: u32,
    aspect: u32,
    mip: u32,
    base_layer: u32,
    layer_count: u32,
    image_offset: [i32; 3],
    image_extent: [u32; 3],
}

#[repr(C)]
struct RenderPassBeginInfo {
    s_type: u32,
    p_next: *const c_void,
    render_pass: RenderPass,
    framebuffer: Framebuffer,
    render_area_offset: [i32; 2],
    render_area_extent: [u32; 2],
    clear_count: u32,
    clears: *const [f32; 4],
}

#[repr(C)]
struct Viewport {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    min_depth: f32,
    max_depth: f32,
}

#[repr(C)]
struct Rect2D {
    offset: [i32; 2],
    extent: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct QueueFamilyProperties {
    queue_flags: u32,
    queue_count: u32,
    timestamp_bits: u32,
    min_transfer: [u32; 3],
}

#[repr(C)]
struct PhysicalDeviceProperties {
    api_version: u32,
    driver_version: u32,
    vendor_id: u32,
    device_id: u32,
    device_type: u32,
    device_name: [u8; 256],
    cache_uuid: [u8; 16],
    limits: [u8; 504],
    sparse: [u8; 20],
}

// MARK: - The function table (one resolve through GetInstanceProcAddr)

type GipaFn = unsafe extern "C" fn(Instance, *const c_char) -> *mut c_void;

type CreateInstanceFn =
    unsafe extern "C" fn(*const InstanceCreateInfo, *const c_void, *mut Instance) -> VkResult;

/// The WSI half of the table - resolvable ONLY on an instance that
/// enabled the surface extensions; the offscreen road never has it.
#[allow(clippy::type_complexity)]
struct WsiFns {
    /// Each door enables only ITS creator's extension — the other one
    /// rightly resolves to nothing, so both are optional and the
    /// install demands only the one it stands in front of.
    create_xcb_surface: Option<
        unsafe extern "C" fn(
            Instance,
            *const XcbSurfaceCreateInfo,
            *const c_void,
            *mut SurfaceKHR,
        ) -> VkResult,
    >,
    create_wayland_surface: Option<
        unsafe extern "C" fn(
            Instance,
            *const WaylandSurfaceCreateInfo,
            *const c_void,
            *mut SurfaceKHR,
        ) -> VkResult,
    >,
    destroy_surface: unsafe extern "C" fn(Instance, SurfaceKHR, *const c_void),
    surface_support: unsafe extern "C" fn(PhysicalDevice, u32, SurfaceKHR, *mut u32) -> VkResult,
    surface_capabilities:
        unsafe extern "C" fn(PhysicalDevice, SurfaceKHR, *mut SurfaceCapabilities) -> VkResult,
    surface_formats:
        unsafe extern "C" fn(PhysicalDevice, SurfaceKHR, *mut u32, *mut SurfaceFormat) -> VkResult,
    surface_present_modes:
        unsafe extern "C" fn(PhysicalDevice, SurfaceKHR, *mut u32, *mut u32) -> VkResult,
    create_swapchain: unsafe extern "C" fn(
        Device,
        *const SwapchainCreateInfo,
        *const c_void,
        *mut SwapchainKHR,
    ) -> VkResult,
    destroy_swapchain: unsafe extern "C" fn(Device, SwapchainKHR, *const c_void),
    get_swapchain_images:
        unsafe extern "C" fn(Device, SwapchainKHR, *mut u32, *mut ImageHandle) -> VkResult,
    acquire_next_image: unsafe extern "C" fn(
        Device,
        SwapchainKHR,
        u64,
        Semaphore,
        Fence,
        *mut u32,
    ) -> VkResult,
    queue_present: unsafe extern "C" fn(Queue, *const PresentInfo) -> VkResult,
}

/// Every entry point this module speaks, resolved once per instance —
/// a strict loader answers instance-level names ONLY with a live
/// instance (and the global trio only without one), so the table
/// resolves after `vkCreateInstance`, never before. A missing symbol
/// refuses the tier (the ladder steps down to gl).
#[allow(clippy::type_complexity)]
struct VkFns {
    destroy_instance: unsafe extern "C" fn(Instance, *const c_void),
    enumerate_physical_devices:
        unsafe extern "C" fn(Instance, *mut u32, *mut PhysicalDevice) -> VkResult,
    get_physical_device_properties:
        unsafe extern "C" fn(PhysicalDevice, *mut PhysicalDeviceProperties),
    get_queue_family_properties:
        unsafe extern "C" fn(PhysicalDevice, *mut u32, *mut QueueFamilyProperties),
    get_memory_properties:
        unsafe extern "C" fn(PhysicalDevice, *mut PhysicalDeviceMemoryProperties),
    create_device: unsafe extern "C" fn(
        PhysicalDevice,
        *const DeviceCreateInfo,
        *const c_void,
        *mut Device,
    ) -> VkResult,
    destroy_device: unsafe extern "C" fn(Device, *const c_void),
    get_device_queue: unsafe extern "C" fn(Device, u32, u32, *mut Queue),
    device_wait_idle: unsafe extern "C" fn(Device) -> VkResult,
    create_image_view: unsafe extern "C" fn(
        Device,
        *const ImageViewCreateInfo,
        *const c_void,
        *mut ImageView,
    ) -> VkResult,
    destroy_image_view: unsafe extern "C" fn(Device, ImageView, *const c_void),
    create_shader_module: unsafe extern "C" fn(
        Device,
        *const ShaderModuleCreateInfo,
        *const c_void,
        *mut ShaderModule,
    ) -> VkResult,
    destroy_shader_module: unsafe extern "C" fn(Device, ShaderModule, *const c_void),
    create_pipeline_layout: unsafe extern "C" fn(
        Device,
        *const PipelineLayoutCreateInfo,
        *const c_void,
        *mut PipelineLayout,
    ) -> VkResult,
    destroy_pipeline_layout: unsafe extern "C" fn(Device, PipelineLayout, *const c_void),
    create_render_pass: unsafe extern "C" fn(
        Device,
        *const RenderPassCreateInfo,
        *const c_void,
        *mut RenderPass,
    ) -> VkResult,
    destroy_render_pass: unsafe extern "C" fn(Device, RenderPass, *const c_void),
    create_graphics_pipelines: unsafe extern "C" fn(
        Device,
        u64,
        u32,
        *const GraphicsPipelineCreateInfo,
        *const c_void,
        *mut Pipeline,
    ) -> VkResult,
    destroy_pipeline: unsafe extern "C" fn(Device, Pipeline, *const c_void),
    create_framebuffer: unsafe extern "C" fn(
        Device,
        *const FramebufferCreateInfo,
        *const c_void,
        *mut Framebuffer,
    ) -> VkResult,
    destroy_framebuffer: unsafe extern "C" fn(Device, Framebuffer, *const c_void),
    create_buffer: unsafe extern "C" fn(
        Device,
        *const BufferCreateInfo,
        *const c_void,
        *mut BufferHandle,
    ) -> VkResult,
    destroy_buffer: unsafe extern "C" fn(Device, BufferHandle, *const c_void),
    get_buffer_memory_requirements:
        unsafe extern "C" fn(Device, BufferHandle, *mut MemoryRequirements),
    get_image_memory_requirements:
        unsafe extern "C" fn(Device, ImageHandle, *mut MemoryRequirements),
    allocate_memory: unsafe extern "C" fn(
        Device,
        *const MemoryAllocateInfo,
        *const c_void,
        *mut DeviceMemory,
    ) -> VkResult,
    free_memory: unsafe extern "C" fn(Device, DeviceMemory, *const c_void),
    bind_buffer_memory: unsafe extern "C" fn(Device, BufferHandle, DeviceMemory, u64) -> VkResult,
    bind_image_memory: unsafe extern "C" fn(Device, ImageHandle, DeviceMemory, u64) -> VkResult,
    map_memory:
        unsafe extern "C" fn(Device, DeviceMemory, u64, u64, u32, *mut *mut c_void) -> VkResult,
    create_image: unsafe extern "C" fn(
        Device,
        *const ImageCreateInfo,
        *const c_void,
        *mut ImageHandle,
    ) -> VkResult,
    destroy_image: unsafe extern "C" fn(Device, ImageHandle, *const c_void),
    create_sampler: unsafe extern "C" fn(
        Device,
        *const SamplerCreateInfo,
        *const c_void,
        *mut Sampler,
    ) -> VkResult,
    destroy_sampler: unsafe extern "C" fn(Device, Sampler, *const c_void),
    create_descriptor_set_layout: unsafe extern "C" fn(
        Device,
        *const DescriptorSetLayoutCreateInfo,
        *const c_void,
        *mut DescriptorSetLayout,
    ) -> VkResult,
    destroy_descriptor_set_layout:
        unsafe extern "C" fn(Device, DescriptorSetLayout, *const c_void),
    create_descriptor_pool: unsafe extern "C" fn(
        Device,
        *const DescriptorPoolCreateInfo,
        *const c_void,
        *mut DescriptorPool,
    ) -> VkResult,
    destroy_descriptor_pool: unsafe extern "C" fn(Device, DescriptorPool, *const c_void),
    allocate_descriptor_sets: unsafe extern "C" fn(
        Device,
        *const DescriptorSetAllocateInfo,
        *mut DescriptorSet,
    ) -> VkResult,
    update_descriptor_sets:
        unsafe extern "C" fn(Device, u32, *const WriteDescriptorSet, u32, *const c_void),
    create_command_pool: unsafe extern "C" fn(
        Device,
        *const CommandPoolCreateInfo,
        *const c_void,
        *mut CommandPool,
    ) -> VkResult,
    destroy_command_pool: unsafe extern "C" fn(Device, CommandPool, *const c_void),
    allocate_command_buffers: unsafe extern "C" fn(
        Device,
        *const CommandBufferAllocateInfo,
        *mut CommandBuffer,
    ) -> VkResult,
    begin_command_buffer:
        unsafe extern "C" fn(CommandBuffer, *const CommandBufferBeginInfo) -> VkResult,
    end_command_buffer: unsafe extern "C" fn(CommandBuffer) -> VkResult,
    reset_command_buffer: unsafe extern "C" fn(CommandBuffer, u32) -> VkResult,
    create_fence:
        unsafe extern "C" fn(Device, *const FenceCreateInfo, *const c_void, *mut Fence) -> VkResult,
    wait_for_fences: unsafe extern "C" fn(Device, u32, *const Fence, u32, u64) -> VkResult,
    reset_fences: unsafe extern "C" fn(Device, u32, *const Fence) -> VkResult,
    create_semaphore: unsafe extern "C" fn(
        Device,
        *const SemaphoreCreateInfo,
        *const c_void,
        *mut Semaphore,
    ) -> VkResult,
    queue_submit: unsafe extern "C" fn(Queue, u32, *const SubmitInfo, Fence) -> VkResult,
    cmd_pipeline_barrier: unsafe extern "C" fn(
        CommandBuffer,
        u32,
        u32,
        u32,
        u32,
        *const c_void,
        u32,
        *const c_void,
        u32,
        *const ImageMemoryBarrier,
    ),
    cmd_copy_buffer_to_image:
        unsafe extern "C" fn(CommandBuffer, BufferHandle, ImageHandle, u32, u32, *const BufferImageCopy),
    cmd_copy_image_to_buffer:
        unsafe extern "C" fn(CommandBuffer, ImageHandle, u32, BufferHandle, u32, *const BufferImageCopy),
    cmd_begin_render_pass:
        unsafe extern "C" fn(CommandBuffer, *const RenderPassBeginInfo, u32),
    cmd_end_render_pass: unsafe extern "C" fn(CommandBuffer),
    cmd_bind_pipeline: unsafe extern "C" fn(CommandBuffer, u32, Pipeline),
    cmd_bind_vertex_buffers:
        unsafe extern "C" fn(CommandBuffer, u32, u32, *const BufferHandle, *const u64),
    cmd_bind_descriptor_sets: unsafe extern "C" fn(
        CommandBuffer,
        u32,
        PipelineLayout,
        u32,
        u32,
        *const DescriptorSet,
        u32,
        *const u32,
    ),
    cmd_push_constants:
        unsafe extern "C" fn(CommandBuffer, PipelineLayout, u32, u32, u32, *const c_void),
    cmd_set_viewport: unsafe extern "C" fn(CommandBuffer, u32, u32, *const Viewport),
    cmd_set_scissor: unsafe extern "C" fn(CommandBuffer, u32, u32, *const Rect2D),
    cmd_draw: unsafe extern "C" fn(CommandBuffer, u32, u32, u32, u32),
}

fn resolve_fns(gipa: GipaFn, instance: Instance) -> Option<VkFns> {
    let sym = |name: &CStr| -> Option<*mut c_void> {
        let address = unsafe { gipa(instance, name.as_ptr()) };
        (!address.is_null()).then_some(address)
    };
    macro_rules! vk {
        ($name:literal) => {
            unsafe { std::mem::transmute(sym($name)?) }
        };
    }
    Some(VkFns {
        destroy_instance: vk!(c"vkDestroyInstance"),
        enumerate_physical_devices: vk!(c"vkEnumeratePhysicalDevices"),
        get_physical_device_properties: vk!(c"vkGetPhysicalDeviceProperties"),
        get_queue_family_properties: vk!(c"vkGetPhysicalDeviceQueueFamilyProperties"),
        get_memory_properties: vk!(c"vkGetPhysicalDeviceMemoryProperties"),
        create_device: vk!(c"vkCreateDevice"),
        destroy_device: vk!(c"vkDestroyDevice"),
        get_device_queue: vk!(c"vkGetDeviceQueue"),
        device_wait_idle: vk!(c"vkDeviceWaitIdle"),
        create_image_view: vk!(c"vkCreateImageView"),
        destroy_image_view: vk!(c"vkDestroyImageView"),
        create_shader_module: vk!(c"vkCreateShaderModule"),
        destroy_shader_module: vk!(c"vkDestroyShaderModule"),
        create_pipeline_layout: vk!(c"vkCreatePipelineLayout"),
        destroy_pipeline_layout: vk!(c"vkDestroyPipelineLayout"),
        create_render_pass: vk!(c"vkCreateRenderPass"),
        destroy_render_pass: vk!(c"vkDestroyRenderPass"),
        create_graphics_pipelines: vk!(c"vkCreateGraphicsPipelines"),
        destroy_pipeline: vk!(c"vkDestroyPipeline"),
        create_framebuffer: vk!(c"vkCreateFramebuffer"),
        destroy_framebuffer: vk!(c"vkDestroyFramebuffer"),
        create_buffer: vk!(c"vkCreateBuffer"),
        destroy_buffer: vk!(c"vkDestroyBuffer"),
        get_buffer_memory_requirements: vk!(c"vkGetBufferMemoryRequirements"),
        get_image_memory_requirements: vk!(c"vkGetImageMemoryRequirements"),
        allocate_memory: vk!(c"vkAllocateMemory"),
        free_memory: vk!(c"vkFreeMemory"),
        bind_buffer_memory: vk!(c"vkBindBufferMemory"),
        bind_image_memory: vk!(c"vkBindImageMemory"),
        map_memory: vk!(c"vkMapMemory"),
        create_image: vk!(c"vkCreateImage"),
        destroy_image: vk!(c"vkDestroyImage"),
        create_sampler: vk!(c"vkCreateSampler"),
        destroy_sampler: vk!(c"vkDestroySampler"),
        create_descriptor_set_layout: vk!(c"vkCreateDescriptorSetLayout"),
        destroy_descriptor_set_layout: vk!(c"vkDestroyDescriptorSetLayout"),
        create_descriptor_pool: vk!(c"vkCreateDescriptorPool"),
        destroy_descriptor_pool: vk!(c"vkDestroyDescriptorPool"),
        allocate_descriptor_sets: vk!(c"vkAllocateDescriptorSets"),
        update_descriptor_sets: vk!(c"vkUpdateDescriptorSets"),
        create_command_pool: vk!(c"vkCreateCommandPool"),
        destroy_command_pool: vk!(c"vkDestroyCommandPool"),
        allocate_command_buffers: vk!(c"vkAllocateCommandBuffers"),
        begin_command_buffer: vk!(c"vkBeginCommandBuffer"),
        end_command_buffer: vk!(c"vkEndCommandBuffer"),
        reset_command_buffer: vk!(c"vkResetCommandBuffer"),
        create_fence: vk!(c"vkCreateFence"),
            wait_for_fences: vk!(c"vkWaitForFences"),
        reset_fences: vk!(c"vkResetFences"),
            create_semaphore: vk!(c"vkCreateSemaphore"),
            queue_submit: vk!(c"vkQueueSubmit"),
        cmd_pipeline_barrier: vk!(c"vkCmdPipelineBarrier"),
        cmd_copy_buffer_to_image: vk!(c"vkCmdCopyBufferToImage"),
        cmd_copy_image_to_buffer: vk!(c"vkCmdCopyImageToBuffer"),
        cmd_begin_render_pass: vk!(c"vkCmdBeginRenderPass"),
        cmd_end_render_pass: vk!(c"vkCmdEndRenderPass"),
        cmd_bind_pipeline: vk!(c"vkCmdBindPipeline"),
        cmd_bind_vertex_buffers: vk!(c"vkCmdBindVertexBuffers"),
        cmd_bind_descriptor_sets: vk!(c"vkCmdBindDescriptorSets"),
        cmd_push_constants: vk!(c"vkCmdPushConstants"),
        cmd_set_viewport: vk!(c"vkCmdSetViewport"),
        cmd_set_scissor: vk!(c"vkCmdSetScissor"),
        cmd_draw: vk!(c"vkCmdDraw"),
    })
}

fn resolve_wsi(gipa: GipaFn, instance: Instance) -> Option<WsiFns> {
    let sym = |name: &CStr| -> Option<*mut c_void> {
        let address = unsafe { gipa(instance, name.as_ptr()) };
        if address.is_null() {
            // name the hole: the ladder's step-down line alone cannot
            // tell a loader quirk from a missing extension
            eprintln!("bunny_ui vk: {} did not resolve", name.to_string_lossy());
        }
        (!address.is_null()).then_some(address)
    };
    macro_rules! vk {
        ($name:literal) => {
            unsafe { std::mem::transmute(sym($name)?) }
        };
    }
    Some(WsiFns {
        create_xcb_surface: {
            let address = unsafe { gipa(instance, c"vkCreateXcbSurfaceKHR".as_ptr()) };
            (!address.is_null()).then(|| unsafe { std::mem::transmute(address) })
        },
        create_wayland_surface: {
            let address = unsafe { gipa(instance, c"vkCreateWaylandSurfaceKHR".as_ptr()) };
            (!address.is_null()).then(|| unsafe { std::mem::transmute(address) })
        },
        destroy_surface: vk!(c"vkDestroySurfaceKHR"),
        surface_support: vk!(c"vkGetPhysicalDeviceSurfaceSupportKHR"),
        surface_capabilities: vk!(c"vkGetPhysicalDeviceSurfaceCapabilitiesKHR"),
        surface_formats: vk!(c"vkGetPhysicalDeviceSurfaceFormatsKHR"),
        surface_present_modes: vk!(c"vkGetPhysicalDeviceSurfacePresentModesKHR"),
        create_swapchain: vk!(c"vkCreateSwapchainKHR"),
        destroy_swapchain: vk!(c"vkDestroySwapchainKHR"),
        get_swapchain_images: vk!(c"vkGetSwapchainImagesKHR"),
        acquire_next_image: vk!(c"vkAcquireNextImageKHR"),
        queue_present: vk!(c"vkQueuePresentKHR"),
    })
}

// MARK: - The loader (dlopen once; the tier refuses without it)

fn loader() -> Option<(GipaFn, CreateInstanceFn)> {
    static LOADED: std::sync::OnceLock<Option<(GipaFn, CreateInstanceFn)>> =
        std::sync::OnceLock::new();
    *LOADED.get_or_init(|| {
        let lib = unsafe { dlopen(c"libvulkan.so.1".as_ptr(), RTLD_NOW) };
        if lib.is_null() {
            return None;
        }
        let gipa = unsafe { dlsym(lib, c"vkGetInstanceProcAddr".as_ptr()) };
        if gipa.is_null() {
            return None;
        }
        let gipa: GipaFn = unsafe { std::mem::transmute(gipa) };
        // ONLY the global command resolves before an instance exists —
        // a strict loader hides everything else until one does
        let create = unsafe { gipa(std::ptr::null_mut(), c"vkCreateInstance".as_ptr()) };
        if create.is_null() {
            return None;
        }
        Some((gipa, unsafe { std::mem::transmute::<*mut c_void, CreateInstanceFn>(create) }))
    })
}

fn ok(result: VkResult) -> bool {
    result == VK_SUCCESS
}

// MARK: - The stack (instance, device, pipelines, fixed state)

/// Which surface the stack will feed — decides extensions and the
/// render pass's final layout.
#[derive(Clone, Copy, PartialEq)]
enum VkTarget {
    WaylandWindow,
    X11Window,
    Offscreen,
}

struct VkStack {
    fns: VkFns,
    /// Present only when the instance enabled the surface extensions.
    wsi: Option<WsiFns>,
    instance: Instance,
    physical: PhysicalDevice,
    device: Device,
    queue: Queue,
    family: u32,
    memory: PhysicalDeviceMemoryProperties,
    sampler: Sampler,
    set_layout: DescriptorSetLayout,
    descriptor_pool: DescriptorPool,
    pipeline_layout: PipelineLayout,
    render_pass: RenderPass,
    rect_pipeline: Pipeline,
    sprite_pipeline: Pipeline,
    mask_pipeline: Pipeline,
    command_pool: CommandPool,
}

fn find_memory_type(
    memory: &PhysicalDeviceMemoryProperties,
    type_bits: u32,
    wanted: u32,
) -> Option<u32> {
    (0..memory.type_count).find(|&index| {
        (type_bits & (1 << index)) != 0
            && memory.types[index as usize].property_flags & wanted == wanted
    })
}

impl VkStack {
    fn create(target: VkTarget) -> Option<VkStack> {
        let result = Self::build(target);
        if let Err(reason) = &result {
            eprintln!("bunny_ui vk: {reason} — stepping down the ladder");
        }
        result.ok()
    }

    fn build(target: VkTarget) -> Result<VkStack, String> {
        let (gipa, create_instance) = loader().ok_or("no libvulkan.so.1 on this system")?;
        unsafe {
            let app = ApplicationInfo {
                s_type: ST_APPLICATION_INFO,
                p_next: std::ptr::null(),
                app_name: c"bunny_ui".as_ptr(),
                app_version: 1,
                engine_name: c"bunny_ui".as_ptr(),
                engine_version: 1,
                api_version: (1 << 22) | (1 << 12), // 1.1 — the floor with WSI everywhere
            };
            let mut extensions: Vec<*const c_char> = Vec::new();
            match target {
                VkTarget::WaylandWindow => {
                    extensions.push(c"VK_KHR_surface".as_ptr());
                    extensions.push(c"VK_KHR_wayland_surface".as_ptr());
                }
                VkTarget::X11Window => {
                    extensions.push(c"VK_KHR_surface".as_ptr());
                    extensions.push(c"VK_KHR_xcb_surface".as_ptr());
                }
                VkTarget::Offscreen => {}
            }
            let create = InstanceCreateInfo {
                s_type: ST_INSTANCE_CREATE,
                p_next: std::ptr::null(),
                flags: 0,
                app_info: &app,
                layer_count: 0,
                layers: std::ptr::null(),
                extension_count: extensions.len() as u32,
                extensions: if extensions.is_empty() {
                    std::ptr::null()
                } else {
                    extensions.as_ptr()
                },
            };
            let mut instance: Instance = std::ptr::null_mut();
            if !ok(create_instance(&create, std::ptr::null(), &mut instance)) {
                return Err("no vulkan instance (missing WSI extension?)".into());
            }
            let fns = resolve_fns(gipa, instance)
                .ok_or("a vulkan symbol is missing from this loader")?;
            let wsi = if target == VkTarget::Offscreen {
                None
            } else {
                Some(resolve_wsi(gipa, instance).ok_or("the WSI symbols are missing")?)
            };
            // the device pick: hardware outranks software by type score
            let mut count = 0;
            (fns.enumerate_physical_devices)(instance, &mut count, std::ptr::null_mut());
            if count == 0 {
                (fns.destroy_instance)(instance, std::ptr::null());
                return Err("no vulkan device at all".into());
            }
            let mut devices = vec![std::ptr::null_mut(); count as usize];
            (fns.enumerate_physical_devices)(instance, &mut count, devices.as_mut_ptr());
            let score_of = |device: PhysicalDevice| -> u32 {
                let mut properties = std::mem::zeroed::<PhysicalDeviceProperties>();
                (fns.get_physical_device_properties)(device, &mut properties);
                match properties.device_type {
                    2 => 4, // discrete
                    1 => 3, // integrated
                    3 => 2, // virtual
                    4 => 1, // cpu (lavapipe counts as came-up — the decision)
                    _ => 0,
                }
            };
            devices.sort_by_key(|&device| std::cmp::Reverse(score_of(device)));
            let mut picked = None;
            for &physical in &devices {
                let mut families = 0;
                (fns.get_queue_family_properties)(physical, &mut families, std::ptr::null_mut());
                let mut properties =
                    vec![std::mem::zeroed::<QueueFamilyProperties>(); families as usize];
                (fns.get_queue_family_properties)(physical, &mut families, properties.as_mut_ptr());
                let family = properties
                    .iter()
                    .position(|family| family.queue_flags & QUEUE_GRAPHICS_BIT != 0);
                if let Some(family) = family {
                    picked = Some((physical, family as u32));
                    break;
                }
            }
            let Some((physical, family)) = picked else {
                (fns.destroy_instance)(instance, std::ptr::null());
                return Err("no graphics queue on any device".into());
            };
            let priority = 1.0f32;
            let queue_info = DeviceQueueCreateInfo {
                s_type: ST_DEVICE_QUEUE_CREATE,
                p_next: std::ptr::null(),
                flags: 0,
                family,
                count: 1,
                priorities: &priority,
            };
            let device_extensions: Vec<*const c_char> = match target {
                VkTarget::Offscreen => vec![],
                _ => vec![c"VK_KHR_swapchain".as_ptr()],
            };
            let device_info = DeviceCreateInfo {
                s_type: ST_DEVICE_CREATE,
                p_next: std::ptr::null(),
                flags: 0,
                queue_count: 1,
                queues: &queue_info,
                layer_count: 0,
                layers: std::ptr::null(),
                extension_count: device_extensions.len() as u32,
                extensions: if device_extensions.is_empty() {
                    std::ptr::null()
                } else {
                    device_extensions.as_ptr()
                },
                features: std::ptr::null(),
            };
            let mut device: Device = std::ptr::null_mut();
            if !ok((fns.create_device)(physical, &device_info, std::ptr::null(), &mut device)) {
                (fns.destroy_instance)(instance, std::ptr::null());
                return Err("the vulkan device refused".into());
            }
            let mut queue: Queue = std::ptr::null_mut();
            (fns.get_device_queue)(device, family, 0, &mut queue);
            let mut memory = std::mem::zeroed::<PhysicalDeviceMemoryProperties>();
            (fns.get_memory_properties)(physical, &mut memory);

            // fixed state: the nearest sampler (texelFetch never uses
            // it, completeness demands it), one sampler-set layout,
            // the shared 56-byte push range, the command pool
            let sampler_info = SamplerCreateInfo {
                s_type: ST_SAMPLER_CREATE,
                p_next: std::ptr::null(),
                flags: 0,
                mag_filter: FILTER_NEAREST,
                min_filter: FILTER_NEAREST,
                mipmap_mode: 0,
                address_u: SAMPLER_ADDRESS_CLAMP_TO_EDGE,
                address_v: SAMPLER_ADDRESS_CLAMP_TO_EDGE,
                address_w: SAMPLER_ADDRESS_CLAMP_TO_EDGE,
                mip_lod_bias: 0.0,
                anisotropy_enable: 0,
                max_anisotropy: 1.0,
                compare_enable: 0,
                compare_op: 0,
                min_lod: 0.0,
                max_lod: 0.0,
                border_color: 0,
                unnormalized: 0,
            };
            let mut sampler: Sampler = 0;
            (fns.create_sampler)(device, &sampler_info, std::ptr::null(), &mut sampler);
            let binding = DescriptorSetLayoutBinding {
                binding: 0,
                descriptor_type: DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER,
                count: 1,
                stages: SHADER_STAGE_FRAGMENT,
                immutable_samplers: std::ptr::null(),
            };
            let layout_info = DescriptorSetLayoutCreateInfo {
                s_type: ST_DESCRIPTOR_SET_LAYOUT_CREATE,
                p_next: std::ptr::null(),
                flags: 0,
                binding_count: 1,
                bindings: &binding,
            };
            let mut set_layout: DescriptorSetLayout = 0;
            (fns.create_descriptor_set_layout)(
                device,
                &layout_info,
                std::ptr::null(),
                &mut set_layout,
            );
            let pool_size = DescriptorPoolSize {
                descriptor_type: DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER,
                count: 64,
            };
            let pool_info = DescriptorPoolCreateInfo {
                s_type: ST_DESCRIPTOR_POOL_CREATE,
                p_next: std::ptr::null(),
                flags: 0,
                max_sets: 64,
                pool_size_count: 1,
                pool_sizes: &pool_size,
            };
            let mut descriptor_pool: DescriptorPool = 0;
            (fns.create_descriptor_pool)(
                device,
                &pool_info,
                std::ptr::null(),
                &mut descriptor_pool,
            );
            let push_range = PushConstantRange {
                stages: SHADER_STAGE_VERTEX | SHADER_STAGE_FRAGMENT,
                offset: 0,
                size: std::mem::size_of::<Push>() as u32,
            };
            let pipeline_layout_info = PipelineLayoutCreateInfo {
                s_type: ST_PIPELINE_LAYOUT_CREATE,
                p_next: std::ptr::null(),
                flags: 0,
                set_layout_count: 1,
                set_layouts: &set_layout,
                push_range_count: 1,
                push_ranges: &push_range,
            };
            let mut pipeline_layout: PipelineLayout = 0;
            (fns.create_pipeline_layout)(
                device,
                &pipeline_layout_info,
                std::ptr::null(),
                &mut pipeline_layout,
            );

            // the one render pass: clear to canvas, store, and end in
            // the layout the target wants back
            let (format, final_layout) = match target {
                VkTarget::Offscreen => (FORMAT_R8G8B8A8_UNORM, IMAGE_LAYOUT_TRANSFER_SRC),
                _ => (FORMAT_B8G8R8A8_UNORM, IMAGE_LAYOUT_PRESENT_SRC_KHR),
            };
            let attachment = AttachmentDescription {
                flags: 0,
                format,
                samples: SAMPLE_COUNT_1,
                load_op: ATTACHMENT_LOAD_CLEAR,
                store_op: ATTACHMENT_STORE_STORE,
                stencil_load: ATTACHMENT_LOAD_DONT_CARE,
                stencil_store: 1,
                initial_layout: IMAGE_LAYOUT_UNDEFINED,
                final_layout,
            };
            let reference =
                AttachmentReference { attachment: 0, layout: IMAGE_LAYOUT_COLOR_ATTACHMENT };
            let subpass = SubpassDescription {
                flags: 0,
                bind_point: PIPELINE_BIND_POINT_GRAPHICS,
                input_count: 0,
                inputs: std::ptr::null(),
                color_count: 1,
                colors: &reference,
                resolves: std::ptr::null(),
                depth: std::ptr::null(),
                preserve_count: 0,
                preserves: std::ptr::null(),
            };
            let pass_info = RenderPassCreateInfo {
                s_type: ST_RENDER_PASS_CREATE,
                p_next: std::ptr::null(),
                flags: 0,
                attachment_count: 1,
                attachments: &attachment,
                subpass_count: 1,
                subpasses: &subpass,
                dependency_count: 0,
                dependencies: std::ptr::null(),
            };
            let mut render_pass: RenderPass = 0;
            (fns.create_render_pass)(device, &pass_info, std::ptr::null(), &mut render_pass);
            let mut command_pool: CommandPool = 0;
            let pool_info = CommandPoolCreateInfo {
                s_type: ST_COMMAND_POOL_CREATE,
                p_next: std::ptr::null(),
                flags: COMMAND_POOL_CREATE_RESET_BUFFER,
                family,
            };
            (fns.create_command_pool)(device, &pool_info, std::ptr::null(), &mut command_pool);
            if sampler == 0
                || set_layout == 0
                || descriptor_pool == 0
                || pipeline_layout == 0
                || render_pass == 0
                || command_pool == 0
            {
                (fns.destroy_device)(device, std::ptr::null());
                (fns.destroy_instance)(instance, std::ptr::null());
                return Err("a vulkan object refused to build".into());
            }
            let mut stack = VkStack {
                fns,
                wsi,
                instance,
                physical,
                device,
                queue,
                family,
                memory,
                sampler,
                set_layout,
                descriptor_pool,
                pipeline_layout,
                render_pass,
                rect_pipeline: 0,
                sprite_pipeline: 0,
                mask_pipeline: 0,
                command_pool,
            };
            stack.build_pipelines()?;
            Ok(stack)
        }
    }

    fn shader(&self, spv: &[u8]) -> Result<ShaderModule, String> {
        // SPIR-V is words; the committed blobs are word-aligned by
        // construction, and the copy below re-aligns defensively
        let words: Vec<u32> = spv
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        let info = ShaderModuleCreateInfo {
            s_type: ST_SHADER_MODULE_CREATE,
            p_next: std::ptr::null(),
            flags: 0,
            code_size: words.len() * 4,
            code: words.as_ptr(),
        };
        let mut module: ShaderModule = 0;
        unsafe {
            if !ok((self.fns.create_shader_module)(
                self.device,
                &info,
                std::ptr::null(),
                &mut module,
            )) {
                return Err("a committed shader module refused".into());
            }
        }
        Ok(module)
    }

    /// One pipeline: the given stages, instance-rate vertex layout,
    /// dynamic viewport/scissor, and the blend the pass wants.
    #[allow(clippy::too_many_arguments)]
    fn pipeline(
        &self,
        vert: ShaderModule,
        frag: ShaderModule,
        bindings: &[VertexInputBinding],
        attributes: &[VertexInputAttribute],
        blend: ColorBlendAttachment,
    ) -> Result<Pipeline, String> {
        let stages = [
            PipelineShaderStage {
                s_type: ST_PIPELINE_SHADER_STAGE,
                p_next: std::ptr::null(),
                flags: 0,
                stage: SHADER_STAGE_VERTEX,
                module: vert,
                name: c"main".as_ptr(),
                specialization: std::ptr::null(),
            },
            PipelineShaderStage {
                s_type: ST_PIPELINE_SHADER_STAGE,
                p_next: std::ptr::null(),
                flags: 0,
                stage: SHADER_STAGE_FRAGMENT,
                module: frag,
                name: c"main".as_ptr(),
                specialization: std::ptr::null(),
            },
        ];
        let vertex_input = PipelineVertexInput {
            s_type: ST_PIPELINE_VERTEX_INPUT,
            p_next: std::ptr::null(),
            flags: 0,
            binding_count: bindings.len() as u32,
            bindings: if bindings.is_empty() { std::ptr::null() } else { bindings.as_ptr() },
            attribute_count: attributes.len() as u32,
            attributes: if attributes.is_empty() {
                std::ptr::null()
            } else {
                attributes.as_ptr()
            },
        };
        let assembly = PipelineInputAssembly {
            s_type: ST_PIPELINE_INPUT_ASSEMBLY,
            p_next: std::ptr::null(),
            flags: 0,
            topology: PRIMITIVE_TOPOLOGY_TRIANGLE_LIST,
            primitive_restart: 0,
        };
        let viewport = PipelineViewportState {
            s_type: ST_PIPELINE_VIEWPORT,
            p_next: std::ptr::null(),
            flags: 0,
            viewport_count: 1,
            viewports: std::ptr::null(),
            scissor_count: 1,
            scissors: std::ptr::null(),
        };
        let rasterization = PipelineRasterization {
            s_type: ST_PIPELINE_RASTERIZATION,
            p_next: std::ptr::null(),
            flags: 0,
            depth_clamp: 0,
            discard: 0,
            polygon_mode: POLYGON_MODE_FILL,
            cull_mode: CULL_MODE_NONE,
            front_face: FRONT_FACE_CCW,
            depth_bias: 0,
            bias_constant: 0.0,
            bias_clamp: 0.0,
            bias_slope: 0.0,
            line_width: 1.0,
        };
        let multisample = PipelineMultisample {
            s_type: ST_PIPELINE_MULTISAMPLE,
            p_next: std::ptr::null(),
            flags: 0,
            samples: SAMPLE_COUNT_1,
            sample_shading: 0,
            min_sample_shading: 0.0,
            sample_mask: std::ptr::null(),
            alpha_to_coverage: 0,
            alpha_to_one: 0,
        };
        let color_blend = PipelineColorBlend {
            s_type: ST_PIPELINE_COLOR_BLEND,
            p_next: std::ptr::null(),
            flags: 0,
            logic_op_enable: 0,
            logic_op: 0,
            attachment_count: 1,
            attachments: &blend,
            blend_constants: [0.0; 4],
        };
        let dynamic_states = [DYNAMIC_STATE_VIEWPORT, DYNAMIC_STATE_SCISSOR];
        let dynamic = PipelineDynamicState {
            s_type: ST_PIPELINE_DYNAMIC,
            p_next: std::ptr::null(),
            flags: 0,
            state_count: 2,
            states: dynamic_states.as_ptr(),
        };
        let info = GraphicsPipelineCreateInfo {
            s_type: ST_GRAPHICS_PIPELINE_CREATE,
            p_next: std::ptr::null(),
            flags: 0,
            stage_count: 2,
            stages: stages.as_ptr(),
            vertex_input: &vertex_input,
            input_assembly: &assembly,
            tessellation: std::ptr::null(),
            viewport: &viewport,
            rasterization: &rasterization,
            multisample: &multisample,
            depth_stencil: std::ptr::null(),
            color_blend: &color_blend,
            dynamic: &dynamic,
            layout: self.pipeline_layout,
            render_pass: self.render_pass,
            subpass: 0,
            base_pipeline: 0,
            base_index: -1,
        };
        let mut pipeline: Pipeline = 0;
        unsafe {
            if !ok((self.fns.create_graphics_pipelines)(
                self.device,
                0,
                1,
                &info,
                std::ptr::null(),
                &mut pipeline,
            )) {
                return Err("a pipeline refused to build".into());
            }
        }
        Ok(pipeline)
    }

    fn build_pipelines(&mut self) -> Result<(), String> {
        // gamma-space source-over with straight alpha — the blend_px law
        let source_over = ColorBlendAttachment {
            blend_enable: 1,
            src_color: BLEND_FACTOR_SRC_ALPHA,
            dst_color: BLEND_FACTOR_ONE_MINUS_SRC_ALPHA,
            color_op: BLEND_OP_ADD,
            src_alpha: BLEND_FACTOR_ONE,
            dst_alpha: BLEND_FACTOR_ONE_MINUS_SRC_ALPHA,
            alpha_op: BLEND_OP_ADD,
            write_mask: 0xF,
        };
        // dst *= src.alpha — the premultiplied corner fade
        let multiply = ColorBlendAttachment {
            blend_enable: 1,
            src_color: BLEND_FACTOR_ZERO,
            dst_color: BLEND_FACTOR_SRC_ALPHA,
            color_op: BLEND_OP_ADD,
            src_alpha: BLEND_FACTOR_ZERO,
            dst_alpha: BLEND_FACTOR_SRC_ALPHA,
            alpha_op: BLEND_OP_ADD,
            write_mask: 0xF,
        };
        let rect_vert = self.shader(RECT_VERT_SPV)?;
        let rect_frag = self.shader(RECT_FRAG_SPV)?;
        let sprite_vert = self.shader(SPRITE_VERT_SPV)?;
        let sprite_frag = self.shader(SPRITE_FRAG_SPV)?;
        let mask_vert = self.shader(MASK_VERT_SPV)?;
        let mask_frag = self.shader(MASK_FRAG_SPV)?;
        // the RectInstance bytes as instance-rate attributes — the
        // same 64-byte lattice every tier shares
        let rect_binding = VertexInputBinding {
            binding: 0,
            stride: std::mem::size_of::<RectInstance>() as u32,
            input_rate: VERTEX_INPUT_RATE_INSTANCE,
        };
        const FORMAT_R8G8B8A8_UNORM_ATTR: u32 = 37;
        let rect_attributes = [
            VertexInputAttribute { location: 0, binding: 0, format: FORMAT_R32G32B32A32_SFLOAT, offset: 0 },
            VertexInputAttribute { location: 1, binding: 0, format: FORMAT_R32G32B32A32_SFLOAT, offset: 16 },
            VertexInputAttribute { location: 2, binding: 0, format: FORMAT_R32G32B32A32_SFLOAT, offset: 32 },
            VertexInputAttribute { location: 3, binding: 0, format: FORMAT_R8G8B8A8_UNORM_ATTR, offset: 48 },
            VertexInputAttribute { location: 4, binding: 0, format: FORMAT_R8G8B8A8_UNORM_ATTR, offset: 52 },
            VertexInputAttribute { location: 5, binding: 0, format: FORMAT_R32G32_SFLOAT, offset: 56 },
            VertexInputAttribute { location: 6, binding: 0, format: FORMAT_R32G32B32A32_SFLOAT, offset: 64 },
        ];
        let sprite_binding = VertexInputBinding {
            binding: 0,
            stride: std::mem::size_of::<SpriteInstance>() as u32,
            input_rate: VERTEX_INPUT_RATE_INSTANCE,
        };
        let sprite_attributes = [
            VertexInputAttribute { location: 0, binding: 0, format: FORMAT_R32G32B32A32_SFLOAT, offset: 0 },
            VertexInputAttribute { location: 1, binding: 0, format: FORMAT_R32G32B32A32_SFLOAT, offset: 16 },
            VertexInputAttribute { location: 2, binding: 0, format: FORMAT_R32G32B32A32_SFLOAT, offset: 32 },
        ];
        self.rect_pipeline = self.pipeline(
            rect_vert,
            rect_frag,
            &[rect_binding],
            &rect_attributes,
            source_over,
        )?;
        self.sprite_pipeline = self.pipeline(
            sprite_vert,
            sprite_frag,
            &[sprite_binding],
            &sprite_attributes,
            source_over,
        )?;
        self.mask_pipeline = self.pipeline(mask_vert, mask_frag, &[], &[], multiply)?;
        unsafe {
            for module in [rect_vert, rect_frag, sprite_vert, sprite_frag, mask_vert, mask_frag] {
                (self.fns.destroy_shader_module)(self.device, module, std::ptr::null());
            }
        }
        Ok(())
    }

    /// A buffer bound to fresh memory; host-visible ones come back
    /// persistently mapped.
    fn buffer(
        &self,
        size: u64,
        usage: u32,
        properties: u32,
    ) -> Option<(BufferHandle, DeviceMemory, *mut u8)> {
        unsafe {
            let info = BufferCreateInfo {
                s_type: ST_BUFFER_CREATE,
                p_next: std::ptr::null(),
                flags: 0,
                size,
                usage,
                sharing: SHARING_MODE_EXCLUSIVE,
                family_count: 0,
                families: std::ptr::null(),
            };
            let mut buffer: BufferHandle = 0;
            if !ok((self.fns.create_buffer)(self.device, &info, std::ptr::null(), &mut buffer)) {
                return None;
            }
            let mut requirements = std::mem::zeroed::<MemoryRequirements>();
            (self.fns.get_buffer_memory_requirements)(self.device, buffer, &mut requirements);
            let memory_type =
                find_memory_type(&self.memory, requirements.memory_type_bits, properties)?;
            let allocate = MemoryAllocateInfo {
                s_type: ST_MEMORY_ALLOCATE,
                p_next: std::ptr::null(),
                size: requirements.size,
                memory_type,
            };
            let mut memory: DeviceMemory = 0;
            if !ok((self.fns.allocate_memory)(
                self.device,
                &allocate,
                std::ptr::null(),
                &mut memory,
            )) {
                (self.fns.destroy_buffer)(self.device, buffer, std::ptr::null());
                return None;
            }
            (self.fns.bind_buffer_memory)(self.device, buffer, memory, 0);
            let mut mapped: *mut c_void = std::ptr::null_mut();
            if properties & MEMORY_PROPERTY_HOST_VISIBLE != 0 {
                (self.fns.map_memory)(self.device, memory, 0, u64::MAX, 0, &mut mapped);
            }
            Some((buffer, memory, mapped as *mut u8))
        }
    }

    /// A sampled+transfer image bound to device-local memory, with its
    /// view and its descriptor set already written.
    fn texture(
        &self,
        width: u32,
        height: u32,
    ) -> Option<(ImageHandle, DeviceMemory, ImageView, DescriptorSet)> {
        unsafe {
            let info = ImageCreateInfo {
                s_type: ST_IMAGE_CREATE,
                p_next: std::ptr::null(),
                flags: 0,
                image_type: IMAGE_TYPE_2D,
                format: FORMAT_R8G8B8A8_UNORM,
                extent: [width, height, 1],
                mip_levels: 1,
                array_layers: 1,
                samples: SAMPLE_COUNT_1,
                tiling: IMAGE_TILING_OPTIMAL,
                usage: IMAGE_USAGE_TRANSFER_DST | IMAGE_USAGE_SAMPLED,
                sharing: SHARING_MODE_EXCLUSIVE,
                family_count: 0,
                families: std::ptr::null(),
                initial_layout: IMAGE_LAYOUT_UNDEFINED,
            };
            let mut image: ImageHandle = 0;
            if !ok((self.fns.create_image)(self.device, &info, std::ptr::null(), &mut image)) {
                return None;
            }
            let mut requirements = std::mem::zeroed::<MemoryRequirements>();
            (self.fns.get_image_memory_requirements)(self.device, image, &mut requirements);
            let memory_type = find_memory_type(
                &self.memory,
                requirements.memory_type_bits,
                MEMORY_PROPERTY_DEVICE_LOCAL,
            )
            .or_else(|| find_memory_type(&self.memory, requirements.memory_type_bits, 0))?;
            let allocate = MemoryAllocateInfo {
                s_type: ST_MEMORY_ALLOCATE,
                p_next: std::ptr::null(),
                size: requirements.size,
                memory_type,
            };
            let mut memory: DeviceMemory = 0;
            if !ok((self.fns.allocate_memory)(
                self.device,
                &allocate,
                std::ptr::null(),
                &mut memory,
            )) {
                (self.fns.destroy_image)(self.device, image, std::ptr::null());
                return None;
            }
            (self.fns.bind_image_memory)(self.device, image, memory, 0);
            let view = self.view_of(image, FORMAT_R8G8B8A8_UNORM)?;
            let allocate = DescriptorSetAllocateInfo {
                s_type: ST_DESCRIPTOR_SET_ALLOCATE,
                p_next: std::ptr::null(),
                pool: self.descriptor_pool,
                count: 1,
                layouts: &self.set_layout,
            };
            let mut set: DescriptorSet = 0;
            if !ok((self.fns.allocate_descriptor_sets)(self.device, &allocate, &mut set)) {
                return None;
            }
            let image_info = DescriptorImageInfo {
                sampler: self.sampler,
                image_view: view,
                layout: IMAGE_LAYOUT_SHADER_READ_ONLY,
            };
            let write = WriteDescriptorSet {
                s_type: ST_WRITE_DESCRIPTOR_SET,
                p_next: std::ptr::null(),
                set,
                binding: 0,
                array_element: 0,
                count: 1,
                descriptor_type: DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER,
                image_info: &image_info,
                buffer_info: std::ptr::null(),
                texel_view: std::ptr::null(),
            };
            (self.fns.update_descriptor_sets)(self.device, 1, &write, 0, std::ptr::null());
            Some((image, memory, view, set))
        }
    }

    fn view_of(&self, image: ImageHandle, format: u32) -> Option<ImageView> {
        let info = ImageViewCreateInfo {
            s_type: ST_IMAGE_VIEW_CREATE,
            p_next: std::ptr::null(),
            flags: 0,
            image,
            view_type: IMAGE_VIEW_TYPE_2D,
            format,
            components: [0; 4],
            aspect: IMAGE_ASPECT_COLOR,
            base_mip: 0,
            mip_count: 1,
            base_layer: 0,
            layer_count: 1,
        };
        let mut view: ImageView = 0;
        unsafe {
            ok((self.fns.create_image_view)(self.device, &info, std::ptr::null(), &mut view))
                .then_some(view)
        }
    }
}

impl Drop for VkStack {
    fn drop(&mut self) {
        // the callers drained the queue before letting go; objects the
        // frames minted (images, buffers) died with their owners
        unsafe {
            (self.fns.device_wait_idle)(self.device);
            (self.fns.destroy_pipeline)(self.device, self.rect_pipeline, std::ptr::null());
            (self.fns.destroy_pipeline)(self.device, self.sprite_pipeline, std::ptr::null());
            (self.fns.destroy_pipeline)(self.device, self.mask_pipeline, std::ptr::null());
            (self.fns.destroy_command_pool)(self.device, self.command_pool, std::ptr::null());
            (self.fns.destroy_render_pass)(self.device, self.render_pass, std::ptr::null());
            (self.fns.destroy_pipeline_layout)(
                self.device,
                self.pipeline_layout,
                std::ptr::null(),
            );
            (self.fns.destroy_descriptor_pool)(
                self.device,
                self.descriptor_pool,
                std::ptr::null(),
            );
            (self.fns.destroy_descriptor_set_layout)(
                self.device,
                self.set_layout,
                std::ptr::null(),
            );
            (self.fns.destroy_sampler)(self.device, self.sampler, std::ptr::null());
            (self.fns.destroy_device)(self.device, std::ptr::null());
            (self.fns.destroy_instance)(self.instance, std::ptr::null());
        }
    }
}

// MARK: - The vulkan ground (staging arena + pending copies)

/// One texture the ground minted: image, memory, view, its descriptor
/// set, and whether the frame recorder has already taken it out of
/// UNDEFINED (the append-only invariant makes later transitions the
/// READ→DST→READ round trip).
struct GroundTexture {
    image: ImageHandle,
    memory: DeviceMemory,
    view: ImageView,
    set: DescriptorSet,
    initialized: bool,
}

/// One tile copy waiting for the frame's command buffer.
struct PendingCopy {
    target: u64,
    region: BufferImageCopy,
}

/// The per-frame upload arena and the texture table. Handles are
/// indices into `textures` (+1 so 0 stays "none"); the shared atlas
/// rides slot `shared`.
struct VkGround {
    textures: HashMap<u64, GroundTexture>,
    next_id: u64,
    shared: Option<u64>,
    /// The staging arena of the ACTIVE slot (bound per present).
    staging_base: *mut u8,
    staging_capacity: usize,
    staging_cursor: usize,
    pending: Vec<PendingCopy>,
    /// Uploads that arrived while staging had no room — the frame
    /// grows the arena and retries (an AtlasFull twin at this level).
    overflow: bool,
}

impl VkGround {
    fn new() -> VkGround {
        VkGround {
            textures: HashMap::new(),
            next_id: 1,
            shared: None,
            staging_base: std::ptr::null_mut(),
            staging_capacity: 0,
            staging_cursor: 0,
            pending: Vec::new(),
            overflow: false,
        }
    }

    fn bind_staging(&mut self, base: *mut u8, capacity: usize) {
        self.staging_base = base;
        self.staging_capacity = capacity;
        self.staging_cursor = 0;
        self.pending.clear();
        self.overflow = false;
    }

    /// Copies `h` rows of `w` pixels (source pitch in pixels) into the
    /// arena tightly and queues the image copy.
    fn stage(&mut self, target: u64, x: u32, y: u32, w: u32, h: u32, bytes: *const u8, pitch_px: u32) {
        let len = (w * h * 4) as usize;
        if self.staging_cursor + len > self.staging_capacity {
            self.overflow = true;
            return;
        }
        let offset = self.staging_cursor;
        unsafe {
            for row in 0..h as usize {
                std::ptr::copy_nonoverlapping(
                    bytes.add(row * pitch_px as usize * 4),
                    self.staging_base.add(offset + row * w as usize * 4),
                    w as usize * 4,
                );
            }
        }
        self.staging_cursor += len;
        self.pending.push(PendingCopy {
            target,
            region: BufferImageCopy {
                buffer_offset: offset as u64,
                buffer_row_length: 0, // tightly packed
                buffer_image_height: 0,
                aspect: IMAGE_ASPECT_COLOR,
                mip: 0,
                base_layer: 0,
                layer_count: 1,
                image_offset: [x as i32, y as i32, 0],
                image_extent: [w, h, 1],
            },
        });
    }
}

/// The ground seam a walk sees, bound to one stack + one VkGround.
struct VkGroundView<'a> {
    stack: &'a VkStack,
    ground: &'a mut VkGround,
}

impl AtlasGround for VkGroundView<'_> {
    fn ensure_shared(&mut self, size: u32) -> bool {
        if self.ground.shared.is_some() {
            return true;
        }
        let Some((image, memory, view, set)) = self.stack.texture(size, size) else {
            return false;
        };
        let id = self.ground.next_id;
        self.ground.next_id += 1;
        self.ground.textures.insert(
            id,
            GroundTexture { image, memory, view, set, initialized: false },
        );
        self.ground.shared = Some(id);
        true
    }

    fn upload_shared(&mut self, x: u32, y: u32, w: u32, h: u32, bytes: *const u8, pitch_px: u32) {
        if let Some(id) = self.ground.shared {
            self.ground.stage(id, x, y, w, h, bytes, pitch_px);
        }
    }

    fn drop_shared(&mut self) {
        if let Some(id) = self.ground.shared.take() {
            if let Some(texture) = self.ground.textures.remove(&id) {
                unsafe {
                    (self.stack.fns.destroy_image_view)(
                        self.stack.device,
                        texture.view,
                        std::ptr::null(),
                    );
                    (self.stack.fns.destroy_image)(
                        self.stack.device,
                        texture.image,
                        std::ptr::null(),
                    );
                    (self.stack.fns.free_memory)(
                        self.stack.device,
                        texture.memory,
                        std::ptr::null(),
                    );
                }
            }
        }
    }

    fn make_dedicated(&mut self, w: u32, h: u32, bytes: &[u8], pitch_px: u32) -> Option<u64> {
        let (image, memory, view, set) = self.stack.texture(w, h)?;
        let id = self.ground.next_id;
        self.ground.next_id += 1;
        self.ground
            .textures
            .insert(id, GroundTexture { image, memory, view, set, initialized: false });
        self.ground.stage(id, 0, 0, w, h, bytes.as_ptr(), pitch_px);
        if self.ground.overflow {
            // the arena had no room — undo the mint, the walk retries
            self.drop_dedicated(id);
            return None;
        }
        Some(id)
    }

    fn drop_dedicated(&mut self, id: u64) {
        if let Some(texture) = self.ground.textures.remove(&id) {
            unsafe {
                (self.stack.fns.destroy_image_view)(
                    self.stack.device,
                    texture.view,
                    std::ptr::null(),
                );
                (self.stack.fns.destroy_image)(self.stack.device, texture.image, std::ptr::null());
                (self.stack.fns.free_memory)(self.stack.device, texture.memory, std::ptr::null());
            }
        }
    }
}

// MARK: - The slot ring (three frames in flight)

const STAGING_INITIAL: usize = 4 * 1024 * 1024;

struct VkSlot {
    command: CommandBuffer,
    fence: Fence,
    acquire: Semaphore,
    render: Semaphore,
    staging: BufferHandle,
    staging_memory: DeviceMemory,
    staging_map: *mut u8,
    staging_capacity: usize,
    rects: BufferHandle,
    rects_memory: DeviceMemory,
    rects_map: *mut u8,
    rects_capacity: usize,
    sprites: BufferHandle,
    sprites_memory: DeviceMemory,
    sprites_map: *mut u8,
    sprites_capacity: usize,
    in_flight: bool,
}

impl VkSlot {
    fn create(stack: &VkStack) -> Option<VkSlot> {
        unsafe {
            let allocate = CommandBufferAllocateInfo {
                s_type: ST_COMMAND_BUFFER_ALLOCATE,
                p_next: std::ptr::null(),
                pool: stack.command_pool,
                level: COMMAND_BUFFER_LEVEL_PRIMARY,
                count: 1,
            };
            let mut command: CommandBuffer = std::ptr::null_mut();
            if !ok((stack.fns.allocate_command_buffers)(stack.device, &allocate, &mut command)) {
                return None;
            }
            let fence_info = FenceCreateInfo {
                s_type: ST_FENCE_CREATE,
                p_next: std::ptr::null(),
                flags: FENCE_CREATE_SIGNALED,
            };
            let mut fence: Fence = 0;
            (stack.fns.create_fence)(stack.device, &fence_info, std::ptr::null(), &mut fence);
            let semaphore_info = SemaphoreCreateInfo {
                s_type: ST_SEMAPHORE_CREATE,
                p_next: std::ptr::null(),
                flags: 0,
            };
            let (mut acquire, mut render): (Semaphore, Semaphore) = (0, 0);
            (stack.fns.create_semaphore)(
                stack.device,
                &semaphore_info,
                std::ptr::null(),
                &mut acquire,
            );
            (stack.fns.create_semaphore)(
                stack.device,
                &semaphore_info,
                std::ptr::null(),
                &mut render,
            );
            let (staging, staging_memory, staging_map) = stack.buffer(
                STAGING_INITIAL as u64,
                BUFFER_USAGE_TRANSFER_SRC,
                MEMORY_PROPERTY_HOST_VISIBLE | MEMORY_PROPERTY_HOST_COHERENT,
            )?;
            if fence == 0 || acquire == 0 || render == 0 {
                return None;
            }
            Some(VkSlot {
                command,
                fence,
                acquire,
                render,
                staging,
                staging_memory,
                staging_map,
                staging_capacity: STAGING_INITIAL,
                rects: 0,
                rects_memory: 0,
                rects_map: std::ptr::null_mut(),
                rects_capacity: 0,
                sprites: 0,
                sprites_memory: 0,
                sprites_map: std::ptr::null_mut(),
                sprites_capacity: 0,
                in_flight: false,
            })
        }
    }

    /// Grows one side (never shrinks); host-visible, persistently
    /// mapped — the ring's fence proved the slot free before this.
    fn ensure_side(
        stack: &VkStack,
        buffer: &mut BufferHandle,
        memory: &mut DeviceMemory,
        map: &mut *mut u8,
        capacity: &mut usize,
        needed: usize,
    ) -> bool {
        if needed == 0 || *capacity >= needed {
            return true;
        }
        unsafe {
            if *buffer != 0 {
                (stack.fns.destroy_buffer)(stack.device, *buffer, std::ptr::null());
                (stack.fns.free_memory)(stack.device, *memory, std::ptr::null());
            }
        }
        let grown = needed.next_multiple_of(64 * 64);
        let Some((new_buffer, new_memory, new_map)) = stack.buffer(
            grown as u64,
            BUFFER_USAGE_VERTEX,
            MEMORY_PROPERTY_HOST_VISIBLE | MEMORY_PROPERTY_HOST_COHERENT,
        ) else {
            return false;
        };
        *buffer = new_buffer;
        *memory = new_memory;
        *map = new_map;
        *capacity = grown;
        true
    }

    fn grow_staging(&mut self, stack: &VkStack, needed: usize) -> bool {
        if self.staging_capacity >= needed {
            return true;
        }
        unsafe {
            (stack.fns.destroy_buffer)(stack.device, self.staging, std::ptr::null());
            (stack.fns.free_memory)(stack.device, self.staging_memory, std::ptr::null());
        }
        let grown = needed.next_power_of_two();
        let Some((buffer, memory, map)) = stack.buffer(
            grown as u64,
            BUFFER_USAGE_TRANSFER_SRC,
            MEMORY_PROPERTY_HOST_VISIBLE | MEMORY_PROPERTY_HOST_COHERENT,
        ) else {
            return false;
        };
        self.staging = buffer;
        self.staging_memory = memory;
        self.staging_map = map;
        self.staging_capacity = grown;
        true
    }
}

fn wait_slot(stack: &VkStack, slot: &mut VkSlot) {
    if slot.in_flight {
        unsafe {
            (stack.fns.wait_for_fences)(stack.device, 1, &slot.fence, 1, u64::MAX);
        }
        slot.in_flight = false;
    }
    unsafe {
        (stack.fns.reset_fences)(stack.device, 1, &slot.fence);
    }
}

fn drain_all(stack: &VkStack, slots: &mut [VkSlot; 3]) {
    for slot in slots.iter_mut() {
        if slot.in_flight {
            unsafe {
                (stack.fns.wait_for_fences)(stack.device, 1, &slot.fence, 1, u64::MAX);
            }
            slot.in_flight = false;
        }
    }
}

// MARK: - The frame recorder (barriers, copies, passes, draws)

/// Records the whole frame into the slot's command buffer: upload
/// barriers and copies first, then the render pass with the runs in
/// paint order, then the optional corner mask.
#[allow(clippy::too_many_arguments)]
fn record_frame(
    stack: &VkStack,
    ground: &mut VkGround,
    slot: &VkSlot,
    framebuffer: Framebuffer,
    extent: (u32, u32),
    canvas: Color,
    batches: &FrameBatches,
    corner_mask: Option<f64>,
) {
    let fns = &stack.fns;
    let command = slot.command;
    unsafe {
        (fns.reset_command_buffer)(command, 0);
        let begin = CommandBufferBeginInfo {
            s_type: ST_COMMAND_BUFFER_BEGIN,
            p_next: std::ptr::null(),
            flags: COMMAND_BUFFER_USAGE_ONE_TIME,
            inheritance: std::ptr::null(),
        };
        (fns.begin_command_buffer)(command, &begin);

        // the upload prologue: each touched image steps into
        // TRANSFER_DST (from UNDEFINED on first use — append-only means
        // a revisit only ever ADDS texels), takes its copies, and steps
        // into SHADER_READ_ONLY for the pass
        let mut touched: Vec<u64> = Vec::new();
        for copy in &ground.pending {
            if !touched.contains(&copy.target) {
                touched.push(copy.target);
            }
        }
        for &id in &touched {
            let Some(texture) = ground.textures.get(&id) else { continue };
            let barrier = ImageMemoryBarrier {
                s_type: ST_IMAGE_MEMORY_BARRIER,
                p_next: std::ptr::null(),
                src_access: if texture.initialized { ACCESS_SHADER_READ } else { 0 },
                dst_access: ACCESS_TRANSFER_WRITE,
                old_layout: if texture.initialized {
                    IMAGE_LAYOUT_SHADER_READ_ONLY
                } else {
                    IMAGE_LAYOUT_UNDEFINED
                },
                new_layout: IMAGE_LAYOUT_TRANSFER_DST,
                src_family: u32::MAX,
                dst_family: u32::MAX,
                image: texture.image,
                aspect: IMAGE_ASPECT_COLOR,
                base_mip: 0,
                mip_count: 1,
                base_layer: 0,
                layer_count: 1,
            };
            (fns.cmd_pipeline_barrier)(
                command,
                PIPELINE_STAGE_TOP | PIPELINE_STAGE_FRAGMENT_SHADER,
                PIPELINE_STAGE_TRANSFER,
                0,
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                1,
                &barrier,
            );
        }
        for copy in &ground.pending {
            let Some(texture) = ground.textures.get(&copy.target) else { continue };
            (fns.cmd_copy_buffer_to_image)(
                command,
                slot.staging,
                texture.image,
                IMAGE_LAYOUT_TRANSFER_DST,
                1,
                &copy.region,
            );
        }
        for &id in &touched {
            let Some(texture) = ground.textures.get_mut(&id) else { continue };
            let barrier = ImageMemoryBarrier {
                s_type: ST_IMAGE_MEMORY_BARRIER,
                p_next: std::ptr::null(),
                src_access: ACCESS_TRANSFER_WRITE,
                dst_access: ACCESS_SHADER_READ,
                old_layout: IMAGE_LAYOUT_TRANSFER_DST,
                new_layout: IMAGE_LAYOUT_SHADER_READ_ONLY,
                src_family: u32::MAX,
                dst_family: u32::MAX,
                image: texture.image,
                aspect: IMAGE_ASPECT_COLOR,
                base_mip: 0,
                mip_count: 1,
                base_layer: 0,
                layer_count: 1,
            };
            (fns.cmd_pipeline_barrier)(
                command,
                PIPELINE_STAGE_TRANSFER,
                PIPELINE_STAGE_FRAGMENT_SHADER,
                0,
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                1,
                &barrier,
            );
            texture.initialized = true;
        }

        // the pass: clear to canvas, the runs in paint order
        let clear = [
            canvas.r as f32 / 255.0,
            canvas.g as f32 / 255.0,
            canvas.b as f32 / 255.0,
            canvas.a as f32 / 255.0,
        ];
        let pass_begin = RenderPassBeginInfo {
            s_type: ST_RENDER_PASS_BEGIN,
            p_next: std::ptr::null(),
            render_pass: stack.render_pass,
            framebuffer,
            render_area_offset: [0, 0],
            render_area_extent: [extent.0, extent.1],
            clear_count: 1,
            clears: &clear,
        };
        (fns.cmd_begin_render_pass)(command, &pass_begin, SUBPASS_CONTENTS_INLINE);
        let viewport = Viewport {
            x: 0.0,
            y: 0.0,
            width: extent.0 as f32,
            height: extent.1 as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        (fns.cmd_set_viewport)(command, 0, 1, &viewport);
        let scissor = Rect2D { offset: [0, 0], extent: [extent.0, extent.1] };
        (fns.cmd_set_scissor)(command, 0, 1, &scissor);

        let mut push = Push {
            round_box: [0.0; 4],
            quad: [0.0; 4],
            round_radii: [0.0; 4],
            viewport: [extent.0 as f32, extent.1 as f32],
        };
        let push_all = |command: CommandBuffer, push: &Push| {
            (fns.cmd_push_constants)(
                command,
                stack.pipeline_layout,
                SHADER_STAGE_VERTEX | SHADER_STAGE_FRAGMENT,
                0,
                std::mem::size_of::<Push>() as u32,
                (push as *const Push).cast(),
            );
        };
        push_all(command, &push);

        let shared_set = ground
            .shared
            .and_then(|id| ground.textures.get(&id))
            .map(|texture| texture.set)
            .unwrap_or(0);
        let mut bound: Option<RunKind> = None;
        let mut bound_round = u32::MAX;
        let mut bound_set: DescriptorSet = 0;
        for run in &batches.runs {
            if bound_round != run.round {
                let round: &RoundClip = &batches.rounds[run.round as usize];
                push.round_box = round.box4;
                push.round_radii = round.radii;
                push_all(command, &push);
                bound_round = run.round;
            }
            let swap = match (bound, run.kind) {
                (Some(RunKind::Sprites | RunKind::Texture(_)), RunKind::Sprites
                | RunKind::Texture(_)) => false,
                (was, now) => was != Some(now),
            };
            if swap {
                let pipeline = match run.kind {
                    RunKind::Rects => stack.rect_pipeline,
                    _ => stack.sprite_pipeline,
                };
                (fns.cmd_bind_pipeline)(command, PIPELINE_BIND_POINT_GRAPHICS, pipeline);
            }
            match run.kind {
                RunKind::Rects => {
                    let offset =
                        run.base as u64 * std::mem::size_of::<RectInstance>() as u64;
                    (fns.cmd_bind_vertex_buffers)(command, 0, 1, &slot.rects, &offset);
                }
                RunKind::Sprites | RunKind::Texture(_) => {
                    let offset =
                        run.base as u64 * std::mem::size_of::<SpriteInstance>() as u64;
                    (fns.cmd_bind_vertex_buffers)(command, 0, 1, &slot.sprites, &offset);
                    let set = match run.kind {
                        RunKind::Texture(index) => batches
                            .textures
                            .get(index as usize)
                            .and_then(|id| ground.textures.get(id))
                            .map(|texture| texture.set)
                            .unwrap_or(shared_set),
                        _ => shared_set,
                    };
                    if set != bound_set && set != 0 {
                        (fns.cmd_bind_descriptor_sets)(
                            command,
                            PIPELINE_BIND_POINT_GRAPHICS,
                            stack.pipeline_layout,
                            0,
                            1,
                            &set,
                            0,
                            std::ptr::null(),
                        );
                        bound_set = set;
                    }
                }
            }
            bound = Some(run.kind);
            (fns.cmd_draw)(command, 6, run.count, 0, 0);
        }

        // the scene's corners: four squares multiplied by the window's
        // own rounded coverage — premultiplied fade, per-pipeline blend
        if let Some(radius) = corner_mask {
            let (w, h) = (extent.0 as f32, extent.1 as f32);
            let r = radius as f32;
            (fns.cmd_bind_pipeline)(command, PIPELINE_BIND_POINT_GRAPHICS, stack.mask_pipeline);
            push.round_box = [0.0, 0.0, w, h];
            push.round_radii = [r; 4];
            for quad in [
                [0.0, 0.0, r, r],
                [w - r, 0.0, w, r],
                [0.0, h - r, r, h],
                [w - r, h - r, w, h],
            ] {
                push.quad = quad;
                push_all(command, &push);
                (fns.cmd_draw)(command, 6, 1, 0, 0);
            }
        }
        (fns.cmd_end_render_pass)(command);
    }
}

/// Uploads the frame's instances into the slot's persistent maps —
/// the fence proved the slot free, so the copies race nothing.
fn upload_instances(stack: &VkStack, slot: &mut VkSlot, batches: &FrameBatches) -> bool {
    let rects_len = std::mem::size_of_val(batches.rects.as_slice());
    let sprites_len = std::mem::size_of_val(batches.sprites.as_slice());
    if !VkSlot::ensure_side(
        stack,
        &mut slot.rects,
        &mut slot.rects_memory,
        &mut slot.rects_map,
        &mut slot.rects_capacity,
        rects_len,
    ) || !VkSlot::ensure_side(
        stack,
        &mut slot.sprites,
        &mut slot.sprites_memory,
        &mut slot.sprites_map,
        &mut slot.sprites_capacity,
        sprites_len,
    ) {
        return false;
    }
    unsafe {
        if rects_len > 0 {
            std::ptr::copy_nonoverlapping(
                batches.rects.as_ptr() as *const u8,
                slot.rects_map,
                rects_len,
            );
        }
        if sprites_len > 0 {
            std::ptr::copy_nonoverlapping(
                batches.sprites.as_ptr() as *const u8,
                slot.sprites_map,
                sprites_len,
            );
        }
    }
    true
}

// MARK: - The swapchain (IMMEDIATE first — pacing stays with the shell)

struct Swapchain {
    handle: SwapchainKHR,
    extent: (u32, u32),
    views: Vec<ImageView>,
    framebuffers: Vec<Framebuffer>,
}

fn build_swapchain(
    stack: &VkStack,
    surface: SurfaceKHR,
    wanted: (u32, u32),
    old: SwapchainKHR,
) -> Option<Swapchain> {
    let fns = &stack.fns;
    let wsi = stack.wsi.as_ref()?;
    unsafe {
        let mut supported = 0u32;
        (wsi.surface_support)(stack.physical, stack.family, surface, &mut supported);
        if supported == 0 {
            return None;
        }
        let mut capabilities = std::mem::zeroed::<SurfaceCapabilities>();
        if !ok((wsi.surface_capabilities)(stack.physical, surface, &mut capabilities)) {
            return None;
        }
        // the format law: B8G8R8A8_UNORM, NEVER sRGB
        let mut format_count = 0;
        (wsi.surface_formats)(stack.physical, surface, &mut format_count, std::ptr::null_mut());
        let mut formats = vec![SurfaceFormat::default(); format_count.max(1) as usize];
        (wsi.surface_formats)(stack.physical, surface, &mut format_count, formats.as_mut_ptr());
        if !formats
            .iter()
            .take(format_count as usize)
            .any(|format| format.format == FORMAT_B8G8R8A8_UNORM)
        {
            eprintln!("bunny_ui vk: no B8G8R8A8_UNORM surface format");
            return None;
        }
        // IMMEDIATE → MAILBOX → FIFO: the first two never block the
        // shell's clocks; FIFO is the lawful last resort everywhere
        let mut mode_count = 0;
        (wsi.surface_present_modes)(
            stack.physical,
            surface,
            &mut mode_count,
            std::ptr::null_mut(),
        );
        let mut modes = vec![0u32; mode_count.max(1) as usize];
        (wsi.surface_present_modes)(stack.physical, surface, &mut mode_count, modes.as_mut_ptr());
        let modes = &modes[..mode_count as usize];
        let present_mode = [PRESENT_MODE_IMMEDIATE, PRESENT_MODE_MAILBOX]
            .into_iter()
            .find(|mode| modes.contains(mode))
            .unwrap_or(PRESENT_MODE_FIFO);
        let extent = if capabilities.current_extent[0] != u32::MAX {
            (capabilities.current_extent[0], capabilities.current_extent[1])
        } else {
            (
                wanted.0.clamp(capabilities.min_extent[0], capabilities.max_extent[0].max(1)),
                wanted.1.clamp(capabilities.min_extent[1], capabilities.max_extent[1].max(1)),
            )
        };
        let min_images = 3
            .max(capabilities.min_image_count)
            .min(if capabilities.max_image_count == 0 {
                u32::MAX
            } else {
                capabilities.max_image_count
            });
        let composite = [COMPOSITE_ALPHA_OPAQUE, COMPOSITE_ALPHA_INHERIT, COMPOSITE_ALPHA_PREMULTIPLIED]
            .into_iter()
            .find(|alpha| capabilities.supported_composite_alpha & alpha != 0)
            .unwrap_or(COMPOSITE_ALPHA_OPAQUE);
        let info = SwapchainCreateInfo {
            s_type: ST_SWAPCHAIN_CREATE_KHR,
            p_next: std::ptr::null(),
            flags: 0,
            surface,
            min_image_count: min_images,
            format: FORMAT_B8G8R8A8_UNORM,
            color_space: COLORSPACE_SRGB_NONLINEAR,
            extent: [extent.0.max(1), extent.1.max(1)],
            array_layers: 1,
            usage: IMAGE_USAGE_COLOR_ATTACHMENT,
            sharing: SHARING_MODE_EXCLUSIVE,
            family_count: 0,
            families: std::ptr::null(),
            pre_transform: capabilities.current_transform,
            composite_alpha: composite,
            present_mode,
            clipped: 1,
            old_swapchain: old,
        };
        let mut handle: SwapchainKHR = 0;
        if !ok((wsi.create_swapchain)(stack.device, &info, std::ptr::null(), &mut handle)) {
            return None;
        }
        let mut image_count = 0;
        (wsi.get_swapchain_images)(stack.device, handle, &mut image_count, std::ptr::null_mut());
        let mut images = vec![0u64; image_count as usize];
        (wsi.get_swapchain_images)(stack.device, handle, &mut image_count, images.as_mut_ptr());
        let mut views = Vec::new();
        let mut framebuffers = Vec::new();
        for &image in &images {
            let view = stack.view_of(image, FORMAT_B8G8R8A8_UNORM)?;
            let framebuffer_info = FramebufferCreateInfo {
                s_type: ST_FRAMEBUFFER_CREATE,
                p_next: std::ptr::null(),
                flags: 0,
                render_pass: stack.render_pass,
                attachment_count: 1,
                attachments: &view,
                width: extent.0.max(1),
                height: extent.1.max(1),
                layers: 1,
            };
            let mut framebuffer: Framebuffer = 0;
            if !ok((fns.create_framebuffer)(
                stack.device,
                &framebuffer_info,
                std::ptr::null(),
                &mut framebuffer,
            )) {
                return None;
            }
            views.push(view);
            framebuffers.push(framebuffer);
        }
        Some(Swapchain { handle, extent, views, framebuffers })
    }
}

fn destroy_swapchain(stack: &VkStack, swapchain: &mut Swapchain) {
    let Some(wsi) = stack.wsi.as_ref() else { return };
    unsafe {
        for &framebuffer in &swapchain.framebuffers {
            (stack.fns.destroy_framebuffer)(stack.device, framebuffer, std::ptr::null());
        }
        for &view in &swapchain.views {
            (stack.fns.destroy_image_view)(stack.device, view, std::ptr::null());
        }
        (wsi.destroy_swapchain)(stack.device, swapchain.handle, std::ptr::null());
    }
    swapchain.framebuffers.clear();
    swapchain.views.clear();
    swapchain.handle = 0;
}

// MARK: - The window presenter

struct VkPresenter {
    stack: VkStack,
    surface: SurfaceKHR,
    swapchain: Swapchain,
    scene: bool,
    slots: [VkSlot; 3],
    cursor: usize,
    atlas: RunAtlas,
    ground: VkGround,
    batches: FrameBatches,
    retained: Option<(DisplayList, (usize, usize), usize, Color)>,
}

fn frame_repeats(
    retained: &Option<(DisplayList, (usize, usize), usize, Color)>,
    display: &DisplayList,
    physical: (usize, usize),
    scale: usize,
    canvas: Color,
) -> bool {
    matches!(retained, Some((list, kept_physical, kept_scale, kept_canvas))
        if *kept_physical == physical
            && *kept_scale == scale
            && *kept_canvas == canvas
            && list.as_slice() == display.as_slice())
}

thread_local! {
    static PRESENTER: RefCell<Option<VkPresenter>> = const { RefCell::new(None) };
    static RECREATE_SPENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[derive(PartialEq)]
enum Presented {
    Ok,
    DeviceLost,
}

impl VkPresenter {
    fn present(
        &mut self,
        display: &DisplayList,
        size: Size,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
    ) -> Presented {
        let physical = (
            (size.width * scale as f64).round().max(0.0) as usize,
            (size.height * scale as f64).round().max(0.0) as usize,
        );
        if physical.0 == 0 || physical.1 == 0 {
            return Presented::Ok;
        }
        if frame_repeats(&self.retained, display, physical, scale, canvas) {
            return Presented::Ok;
        }
        // the slot first: its fence guards the staging arena the walk
        // is about to fill
        let index = self.cursor;
        self.cursor = (self.cursor + 1) % self.slots.len();
        wait_slot(&self.stack, &mut self.slots[index]);
        // walk with retries; staging overflow grows the arena, atlas
        // overflow runs the copying collector
        for attempt in 0..4 {
            self.ground
                .bind_staging(self.slots[index].staging_map, self.slots[index].staging_capacity);
            let mut view = VkGroundView { stack: &self.stack, ground: &mut self.ground };
            let walked = build_frame(
                &mut view,
                display,
                scale,
                physical,
                text,
                images,
                &mut self.atlas,
                &mut self.batches,
            );
            match walked {
                Ok(()) if !self.ground.overflow => break,
                Ok(()) => {
                    let needed = self.ground.staging_capacity * 2;
                    drain_all(&self.stack, &mut self.slots);
                    if !self.slots[index].grow_staging(&self.stack, needed) {
                        return Presented::DeviceLost;
                    }
                }
                Err(AtlasFull) => {
                    if attempt == 3 {
                        eprintln!("bunny_ui vk: atlas overflow survived the resets");
                        break;
                    }
                    drain_all(&self.stack, &mut self.slots);
                    let mut view =
                        VkGroundView { stack: &self.stack, ground: &mut self.ground };
                    self.atlas.reset(&mut view, true);
                }
            }
        }
        if !upload_instances(&self.stack, &mut self.slots[index], &self.batches) {
            return Presented::DeviceLost;
        }
        // acquire; a stale swapchain rebuilds once and retries
        let mut image_index = 0u32;
        for attempt in 0..2 {
            let wsi = self.stack.wsi.as_ref().expect("a window stack keeps its WSI");
            let acquired = unsafe {
                (wsi.acquire_next_image)(
                    self.stack.device,
                    self.swapchain.handle,
                    u64::MAX,
                    self.slots[index].acquire,
                    0,
                    &mut image_index,
                )
            };
            match acquired {
                VK_SUCCESS | VK_SUBOPTIMAL_KHR => break,
                VK_ERROR_OUT_OF_DATE_KHR | VK_ERROR_SURFACE_LOST_KHR if attempt == 0 => {
                    if !self.recreate_swapchain(physical) {
                        return Presented::DeviceLost;
                    }
                }
                VK_ERROR_DEVICE_LOST => return Presented::DeviceLost,
                _ => return Presented::Ok,
            }
        }
        let corner_mask = self.scene.then_some(8.0 * scale as f64);
        record_frame(
            &self.stack,
            &mut self.ground,
            &self.slots[index],
            self.swapchain.framebuffers[image_index as usize],
            self.swapchain.extent,
            canvas,
            &self.batches,
            corner_mask,
        );
        unsafe {
            (self.stack.fns.end_command_buffer)(self.slots[index].command);
            let wait_stage = PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT;
            let submit = SubmitInfo {
                s_type: ST_SUBMIT_INFO,
                p_next: std::ptr::null(),
                wait_count: 1,
                wait_semaphores: &self.slots[index].acquire,
                wait_stages: &wait_stage,
                command_buffer_count: 1,
                command_buffers: &self.slots[index].command,
                signal_count: 1,
                signal_semaphores: &self.slots[index].render,
            };
            if !ok((self.stack.fns.queue_submit)(
                self.stack.queue,
                1,
                &submit,
                self.slots[index].fence,
            )) {
                return Presented::DeviceLost;
            }
            self.slots[index].in_flight = true;
            // the same envelope every tier wears: the shell's
            // pre-present rides before the WSI commit, the note after
            if !crate::ffi::gpu_pre_present(scale) {
                return Presented::Ok;
            }
            let present = PresentInfo {
                s_type: ST_PRESENT_INFO_KHR,
                p_next: std::ptr::null(),
                wait_count: 1,
                wait_semaphores: &self.slots[index].render,
                swapchain_count: 1,
                swapchains: &self.swapchain.handle,
                image_indices: &image_index,
                results: std::ptr::null_mut(),
            };
            let wsi = self.stack.wsi.as_ref().expect("a window stack keeps its WSI");
            let presented = (wsi.queue_present)(self.stack.queue, &present);
            match presented {
                VK_SUCCESS => {}
                VK_SUBOPTIMAL_KHR | VK_ERROR_OUT_OF_DATE_KHR => {
                    // accepted or stale — the next frame rebuilds
                    let _ = self.recreate_swapchain(physical);
                }
                VK_ERROR_DEVICE_LOST => return Presented::DeviceLost,
                _ => return Presented::Ok,
            }
        }
        crate::ffi::gpu_note_present();
        self.retained = Some((display.clone(), physical, scale, canvas));
        Presented::Ok
    }

    fn recreate_swapchain(&mut self, wanted: (usize, usize)) -> bool {
        unsafe {
            (self.stack.fns.device_wait_idle)(self.stack.device);
        }
        for slot in &mut self.slots {
            slot.in_flight = false;
        }
        let old = self.swapchain.handle;
        let Some(fresh) = build_swapchain(
            &self.stack,
            self.surface,
            (wanted.0 as u32, wanted.1 as u32),
            old,
        ) else {
            return false;
        };
        destroy_swapchain(&self.stack, &mut self.swapchain);
        self.swapchain = fresh;
        self.retained = None;
        true
    }
}

// MARK: - Install and the ladder

fn install() -> Option<VkPresenter> {
    use crate::ffi::GpuTargets;
    let targets = crate::ffi::gpu_targets()?;
    let (target, scene) = match &targets {
        GpuTargets::Wayland { scene, .. } => (VkTarget::WaylandWindow, *scene),
        GpuTargets::X11 { scene, .. } => (VkTarget::X11Window, *scene),
    };
    let stack = VkStack::create(target)?;
    let surface = unsafe {
        let mut surface: SurfaceKHR = 0;
        let made = match &targets {
            GpuTargets::Wayland { display, surface: wl_surface, .. } => {
                let info = WaylandSurfaceCreateInfo {
                    s_type: ST_WAYLAND_SURFACE_CREATE_KHR,
                    p_next: std::ptr::null(),
                    flags: 0,
                    display: *display,
                    surface: *wl_surface,
                };
                let Some(create) =
                    stack.wsi.as_ref().expect("window WSI").create_wayland_surface
                else {
                    eprintln!("bunny_ui vk: no wayland WSI — stepping down the ladder");
                    return None;
                };
                create(stack.instance, &info, std::ptr::null(), &mut surface)
            }
            GpuTargets::X11 { connection, window, .. } => {
                let info = XcbSurfaceCreateInfo {
                    s_type: ST_XCB_SURFACE_CREATE_KHR,
                    p_next: std::ptr::null(),
                    flags: 0,
                    connection: *connection,
                    window: *window,
                };
                let Some(create) = stack.wsi.as_ref().expect("window WSI").create_xcb_surface
                else {
                    eprintln!("bunny_ui vk: no xcb WSI — stepping down the ladder");
                    return None;
                };
                create(stack.instance, &info, std::ptr::null(), &mut surface)
            }
        };
        if !ok(made) {
            eprintln!("bunny_ui vk: no WSI surface — stepping down the ladder");
            return None;
        }
        surface
    };
    let (width, height) = crate::ffi::gpu_buffer_size();
    let Some(swapchain) = build_swapchain(&stack, surface, (width as u32, height as u32), 0)
    else {
        unsafe { (stack.wsi.as_ref().expect("window WSI").destroy_surface)(stack.instance, surface, std::ptr::null()) };
        eprintln!("bunny_ui vk: no swapchain — stepping down the ladder");
        return None;
    };
    let slots = [
        VkSlot::create(&stack)?,
        VkSlot::create(&stack)?,
        VkSlot::create(&stack)?,
    ];
    Some(VkPresenter {
        stack,
        surface,
        swapchain,
        scene,
        slots,
        cursor: 0,
        atlas: RunAtlas::new(),
        ground: VkGround::new(),
        batches: FrameBatches::default(),
        retained: None,
    })
}

/// The front of the ladder: vulkan if it fully comes up, else the
/// caller steps down to gl. `BUNNY_PRESENT=gl|cpu` skips this tier
/// before any loader touch.
pub(crate) fn try_install() -> bool {
    match std::env::var("BUNNY_PRESENT").ok().as_deref() {
        Some("cpu") | Some("gl") => return false,
        _ => {}
    }
    let Some(presenter) = install() else {
        return false;
    };
    PRESENTER.with(|slot| *slot.borrow_mut() = Some(presenter));
    true
}

pub(crate) fn active() -> bool {
    PRESENTER.with(|slot| slot.borrow().is_some())
}

/// The ack road's skip-breaker, same contract as the gl tier's.
pub(crate) fn invalidate() {
    PRESENTER.with(|slot| {
        if let Some(presenter) = slot.borrow_mut().as_mut() {
            presenter.retained = None;
        }
    });
}

pub(crate) fn present_window(
    display: &DisplayList,
    size: Size,
    scale: usize,
    canvas: Color,
    text: &dyn TextEngine,
    images: &dyn ImageEngine,
) {
    let outcome = PRESENTER.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map(|presenter| presenter.present(display, size, scale, canvas, text, images))
    });
    if outcome != Some(Presented::DeviceLost) {
        return;
    }
    teardown();
    if !RECREATE_SPENT.with(|spent| spent.replace(true)) {
        if let Some(mut presenter) = install() {
            presenter.present(display, size, scale, canvas, text, images);
            PRESENTER.with(|slot| *slot.borrow_mut() = Some(presenter));
            return;
        }
    }
    // the gl tier below catches the window for the rest of its life
    eprintln!("bunny_ui vk: the device is lost — stepping down the ladder");
    let _ = crate::gl::try_install();
}

pub(crate) fn teardown() {
    PRESENTER.with(|slot| {
        let Some(mut presenter) = slot.borrow_mut().take() else { return };
        unsafe {
            (presenter.stack.fns.device_wait_idle)(presenter.stack.device);
        }
        destroy_swapchain(&presenter.stack, &mut presenter.swapchain);
        unsafe {
            (presenter.stack.wsi.as_ref().expect("window WSI").destroy_surface)(
                presenter.stack.instance,
                presenter.surface,
                std::ptr::null(),
            );
        }
        // slots and ground die with the device in the stack's Drop
    });
}

// MARK: - Offscreen target (parity tests and the bench)

/// A windowless render target: same stack, same pipelines, an image
/// whose readback lines up with the CPU mirror byte for byte — and on
/// this tier the rows come back TOP-FIRST already (NDC points down).
pub struct OffscreenVk {
    stack: VkStack,
    target: ImageHandle,
    target_memory: DeviceMemory,
    framebuffer: Framebuffer,
    readback: BufferHandle,
    readback_memory: DeviceMemory,
    readback_map: *mut u8,
    width: usize,
    height: usize,
    slots: [VkSlot; 3],
    cursor: usize,
    atlas: RunAtlas,
    ground: VkGround,
    batches: FrameBatches,
}

impl OffscreenVk {
    pub fn new(width: usize, height: usize) -> Option<OffscreenVk> {
        if width == 0 || height == 0 {
            return None;
        }
        let stack = VkStack::create(VkTarget::Offscreen)?;
        let (target, target_memory, view) = unsafe {
            let info = ImageCreateInfo {
                s_type: ST_IMAGE_CREATE,
                p_next: std::ptr::null(),
                flags: 0,
                image_type: IMAGE_TYPE_2D,
                format: FORMAT_R8G8B8A8_UNORM,
                extent: [width as u32, height as u32, 1],
                mip_levels: 1,
                array_layers: 1,
                samples: SAMPLE_COUNT_1,
                tiling: IMAGE_TILING_OPTIMAL,
                usage: IMAGE_USAGE_COLOR_ATTACHMENT | IMAGE_USAGE_TRANSFER_SRC,
                sharing: SHARING_MODE_EXCLUSIVE,
                family_count: 0,
                families: std::ptr::null(),
                initial_layout: IMAGE_LAYOUT_UNDEFINED,
            };
            let mut image: ImageHandle = 0;
            if !ok((stack.fns.create_image)(stack.device, &info, std::ptr::null(), &mut image)) {
                return None;
            }
            let mut requirements = std::mem::zeroed::<MemoryRequirements>();
            (stack.fns.get_image_memory_requirements)(stack.device, image, &mut requirements);
            let memory_type = find_memory_type(
                &stack.memory,
                requirements.memory_type_bits,
                MEMORY_PROPERTY_DEVICE_LOCAL,
            )
            .or_else(|| find_memory_type(&stack.memory, requirements.memory_type_bits, 0))?;
            let allocate = MemoryAllocateInfo {
                s_type: ST_MEMORY_ALLOCATE,
                p_next: std::ptr::null(),
                size: requirements.size,
                memory_type,
            };
            let mut memory: DeviceMemory = 0;
            if !ok((stack.fns.allocate_memory)(
                stack.device,
                &allocate,
                std::ptr::null(),
                &mut memory,
            )) {
                return None;
            }
            (stack.fns.bind_image_memory)(stack.device, image, memory, 0);
            let view = stack.view_of(image, FORMAT_R8G8B8A8_UNORM)?;
            (image, memory, view)
        };
        let framebuffer = unsafe {
            let info = FramebufferCreateInfo {
                s_type: ST_FRAMEBUFFER_CREATE,
                p_next: std::ptr::null(),
                flags: 0,
                render_pass: stack.render_pass,
                attachment_count: 1,
                attachments: &view,
                width: width as u32,
                height: height as u32,
                layers: 1,
            };
            let mut framebuffer: Framebuffer = 0;
            if !ok((stack.fns.create_framebuffer)(
                stack.device,
                &info,
                std::ptr::null(),
                &mut framebuffer,
            )) {
                return None;
            }
            framebuffer
        };
        let (readback, readback_memory, readback_map) = stack.buffer(
            (width * height * 4) as u64,
            BUFFER_USAGE_TRANSFER_DST,
            MEMORY_PROPERTY_HOST_VISIBLE | MEMORY_PROPERTY_HOST_COHERENT,
        )?;
        let slots = [
            VkSlot::create(&stack)?,
            VkSlot::create(&stack)?,
            VkSlot::create(&stack)?,
        ];
        Some(OffscreenVk {
            stack,
            target,
            target_memory,
            framebuffer,
            readback,
            readback_memory,
            readback_map,
            width,
            height,
            slots,
            cursor: 0,
            atlas: RunAtlas::new(),
            ground: VkGround::new(),
            batches: FrameBatches::default(),
        })
    }

    fn present_inner(
        &mut self,
        display: &DisplayList,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
        wait: bool,
    ) {
        let index = self.cursor;
        self.cursor = (self.cursor + 1) % self.slots.len();
        wait_slot(&self.stack, &mut self.slots[index]);
        for attempt in 0..4 {
            self.ground
                .bind_staging(self.slots[index].staging_map, self.slots[index].staging_capacity);
            let mut view = VkGroundView { stack: &self.stack, ground: &mut self.ground };
            let walked = build_frame(
                &mut view,
                display,
                scale,
                (self.width, self.height),
                text,
                images,
                &mut self.atlas,
                &mut self.batches,
            );
            match walked {
                Ok(()) if !self.ground.overflow => break,
                Ok(()) => {
                    let needed = self.ground.staging_capacity * 2;
                    drain_all(&self.stack, &mut self.slots);
                    if !self.slots[index].grow_staging(&self.stack, needed) {
                        return;
                    }
                }
                Err(AtlasFull) => {
                    if attempt == 3 {
                        eprintln!("bunny_ui vk: atlas overflow survived the resets");
                        break;
                    }
                    drain_all(&self.stack, &mut self.slots);
                    let mut view =
                        VkGroundView { stack: &self.stack, ground: &mut self.ground };
                    self.atlas.reset(&mut view, true);
                }
            }
        }
        if !upload_instances(&self.stack, &mut self.slots[index], &self.batches) {
            return;
        }
        record_frame(
            &self.stack,
            &mut self.ground,
            &self.slots[index],
            self.framebuffer,
            (self.width as u32, self.height as u32),
            canvas,
            &self.batches,
            None,
        );
        // the readback tail rides the same command buffer: the pass
        // ended in TRANSFER_SRC (the offscreen final layout) — copy
        // into the host buffer before the fence signals
        unsafe {
            let region = BufferImageCopy {
                buffer_offset: 0,
                buffer_row_length: 0,
                buffer_image_height: 0,
                aspect: IMAGE_ASPECT_COLOR,
                mip: 0,
                base_layer: 0,
                layer_count: 1,
                image_offset: [0, 0, 0],
                image_extent: [self.width as u32, self.height as u32, 1],
            };
            (self.stack.fns.cmd_copy_image_to_buffer)(
                self.slots[index].command,
                self.target,
                IMAGE_LAYOUT_TRANSFER_SRC,
                self.readback,
                1,
                &region,
            );
            (self.stack.fns.end_command_buffer)(self.slots[index].command);
            let submit = SubmitInfo {
                s_type: ST_SUBMIT_INFO,
                p_next: std::ptr::null(),
                wait_count: 0,
                wait_semaphores: std::ptr::null(),
                wait_stages: std::ptr::null(),
                command_buffer_count: 1,
                command_buffers: &self.slots[index].command,
                signal_count: 0,
                signal_semaphores: std::ptr::null(),
            };
            (self.stack.fns.queue_submit)(
                self.stack.queue,
                1,
                &submit,
                self.slots[index].fence,
            );
            self.slots[index].in_flight = true;
            if wait {
                (self.stack.fns.wait_for_fences)(
                    self.stack.device,
                    1,
                    &self.slots[index].fence,
                    1,
                    u64::MAX,
                );
                self.slots[index].in_flight = false;
            }
        }
    }

    pub fn present_wait(
        &mut self,
        display: &DisplayList,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
    ) {
        self.present_inner(display, scale, canvas, text, images, true);
    }

    pub fn present_nowait(
        &mut self,
        display: &DisplayList,
        scale: usize,
        canvas: Color,
        text: &dyn TextEngine,
        images: &dyn ImageEngine,
    ) {
        self.present_inner(display, scale, canvas, text, images, false);
    }

    #[cfg(test)]
    fn atlas_footprint(&self) -> (usize, u32) {
        self.atlas.footprint()
    }

    /// The rendered bytes, R,G,B,A per pixel — rows already top-first
    /// (this tier's NDC points down, the raster's own direction).
    pub fn read_rgba(&self) -> Vec<u8> {
        let len = self.width * self.height * 4;
        let mut bytes = vec![0u8; len];
        unsafe {
            std::ptr::copy_nonoverlapping(self.readback_map, bytes.as_mut_ptr(), len);
        }
        bytes
    }
}

impl Drop for OffscreenVk {
    fn drop(&mut self) {
        drain_all(&self.stack, &mut self.slots);
        unsafe {
            (self.stack.fns.destroy_framebuffer)(
                self.stack.device,
                self.framebuffer,
                std::ptr::null(),
            );
            (self.stack.fns.destroy_image)(self.stack.device, self.target, std::ptr::null());
            (self.stack.fns.free_memory)(self.stack.device, self.target_memory, std::ptr::null());
            (self.stack.fns.destroy_buffer)(self.stack.device, self.readback, std::ptr::null());
            (self.stack.fns.free_memory)(
                self.stack.device,
                self.readback_memory,
                std::ptr::null(),
            );
        }
    }
}

// MARK: - Tests

#[cfg(test)]
mod tests {
    use super::*;
    use bunny_ui::prelude::*;
    use bunny_ui::raster::rasterize_with;

    /// One probe, cached: llvmpipe makes a context near-universal, but
    /// a machine without libEGL skips honestly.
    fn device_present() -> bool {
        static PRESENT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let present = *PRESENT.get_or_init(|| OffscreenVk::new(4, 4).is_some());
        if !present {
            eprintln!("no vulkan device — skipping");
        }
        present
    }

    /// Renders the same scene by both backends: the GPU offscreen target
    /// and the CPU raster oracle, byte-comparable RGBA out of each.
    fn scene_bytes(
        root: &impl View,
        logical: Size,
        scale: usize,
        canvas: Color,
    ) -> (Vec<u8>, Vec<u8>) {
        let physical = (
            (logical.width.round() as usize) * scale,
            (logical.height.round() as usize) * scale,
        );
        let runtime = Runtime::new();
        let display = runtime.display_frame(root, logical);
        let cpu = rasterize_with(
            &display,
            physical.0,
            physical.1,
            scale,
            canvas,
            &PixelFont,
            &RawImages::default(),
        )
        .to_rgba_bytes();
        let mut gpu = OffscreenVk::new(physical.0, physical.1).expect("offscreen gpu");
        gpu.present_wait(&display, scale, canvas, &PixelFont, &RawImages::default());
        (gpu.read_rgba(), cpu)
    }

    fn max_channel_delta(a: &[u8], b: &[u8]) -> u8 {
        a.iter().zip(b.iter()).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)
    }

    /// The parity gate for anti-aliased scenes: every channel within
    /// `max_delta`, and at most 1% of channels beyond one step (float
    /// coverage vs the cpu's two integer roundings).
    fn assert_close(gpu: &[u8], cpu: &[u8], max_delta: u8, label: &str) {
        assert_eq!(gpu.len(), cpu.len(), "{label}: byte lengths differ");
        let mut worst = 0u8;
        let mut beyond_one = 0usize;
        for (a, b) in gpu.iter().zip(cpu.iter()) {
            let delta = a.abs_diff(*b);
            worst = worst.max(delta);
            if delta > 1 {
                beyond_one += 1;
            }
        }
        assert!(worst <= max_delta, "{label}: worst channel delta {worst} (allowed {max_delta})");
        let share = beyond_one as f64 / gpu.len() as f64;
        assert!(
            share <= 0.01,
            "{label}: {beyond_one} channels beyond one step ({:.3}% > 1%)",
            share * 100.0
        );
    }

    #[test]
    fn a_clear_frame_reads_back_the_canvas_color_exactly() {
        if !device_present() {
            return;
        }
        // this test is the ABI smoke: every resolved symbol in the
        // present path runs once — a wrong signature corrupts the
        // readback loudly (the lesson the text engine taught)
        let canvas = Color::hex(0x18181D);
        let mut gpu = OffscreenVk::new(16, 16).expect("offscreen gpu");
        gpu.present_wait(&DisplayList::default(), 2, canvas, &PixelFont, &RawImages::default());
        let bytes = gpu.read_rgba();
        assert_eq!(bytes.len(), 16 * 16 * 4);
        for pixel in bytes.chunks_exact(4) {
            assert_eq!(pixel, [0x18, 0x18, 0x1D, 0xFF]);
        }
    }

    #[test]
    fn flat_opaque_rects_match_byte_for_byte() {
        if !device_present() {
            return;
        }
        let root = vstack((
            empty().frame(120.0, 40.0).background_color(Color::hex(0x3B82F6)),
            empty()
                .frame(80.0, 24.0)
                .background_color(Color::hex(0x18181D))
                .padding_length(10.0)
                .background_color(Color::hex(0xDDE1E9)),
        ));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 200.0, height: 120.0 }, 2, Color::CANVAS);
        assert!(
            gpu == cpu,
            "flat opaque scene diverged (max channel delta {})",
            max_channel_delta(&gpu, &cpu)
        );
    }

    #[test]
    fn translucent_veils_match_within_one() {
        if !device_present() {
            return;
        }
        // stacked veils: float source-over vs the CPU's rounded div255 —
        // at most one step apart, per channel
        let veil = Color::rgba(0, 0, 0, 90);
        let root = zstack((
            empty().frame(140.0, 100.0).background_color(Color::hex(0xE7EAF1)),
            empty().frame(100.0, 70.0).background_color(veil),
            empty().frame(60.0, 40.0).background_color(veil),
        ));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 160.0, height: 120.0 }, 2, Color::CANVAS);
        let delta = max_channel_delta(&gpu, &cpu);
        assert!(delta <= 1, "veils drifted by {delta} (allowed 1)");
    }

    #[test]
    fn nested_clips_cut_identically() {
        if !device_present() {
            return;
        }
        // a list taller than its frame: the scroll clip cuts rows and
        // the translucent scrollbar rides on top
        let rows: Vec<usize> = (0..12).collect();
        let root = list(rows, |row| row.to_string(), |row| {
            let tint = if row % 2 == 0 { Color::hex(0x3B82F6) } else { Color::hex(0xDDE1E9) };
            empty().frame(160.0, 24.0).background_color(tint)
        })
        .frame(180.0, 100.0);
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 200.0, height: 120.0 }, 2, Color::CANVAS);
        let delta = max_channel_delta(&gpu, &cpu);
        assert!(delta <= 1, "clipped scene drifted by {delta} (allowed 1)");
    }

    /// The whole front on one screen: a bordered rounded island with
    /// .clipped(), text crossing its corner, a child panel with its
    /// own background, and a SCROLL nested inside — rects, sprites and
    /// the inheritance rule all under the AA gate at once.
    #[test]
    fn a_rounded_clip_cuts_the_same_corner_on_both_backends() {
        if !device_present() {
            return;
        }
        let rows: Vec<usize> = (0..8).collect();
        let root = vstack((
            text("corner text").foreground_color(Color::hex(0x202531)),
            empty().frame(150.0, 18.0).background_color(Color::hex(0xAA3322)),
            list(rows, |row| row.to_string(), |row| {
                let tint =
                    if row % 2 == 0 { Color::hex(0x3B82F6) } else { Color::hex(0xDDE1E9) };
                empty().frame(150.0, 16.0).background_color(tint)
            })
            .frame(160.0, 60.0),
        ))
        .background_color(Color::hex(0xF0F2F6))
        .border(Color::hex(0x202531), 1.0)
        .corner_radius(10.0)
        .clipped()
        .frame(170.0, 120.0);
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 190.0, height: 140.0 }, 2, Color::CANVAS);
        assert_close(&gpu, &cpu, 2, "rounded clip");
    }

    /// Radius zero through the whole new plumbing: the strict tier
    /// must not move — the curve is exactly nothing when absent.
    #[test]
    fn a_clipped_box_without_a_radius_stays_strict() {
        if !device_present() {
            return;
        }
        let root = vstack((
            text("square").foreground_color(Color::hex(0x202531)),
            empty().frame(120.0, 20.0).background_color(Color::hex(0x3B82F6)),
        ))
        .background_color(Color::hex(0xF0F2F6))
        .clipped()
        .frame(140.0, 34.0);
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 160.0, height: 60.0 }, 2, Color::CANVAS);
        let delta = max_channel_delta(&gpu, &cpu);
        assert!(delta <= 1, "the straight cut drifted by {delta} (allowed 1)");
    }

    #[test]
    fn rounded_fill_within_tolerance() {
        if !device_present() {
            return;
        }
        // the finder radius and an exaggerated one, on a dark canvas —
        // the corner-bug configuration, judged by the oracle
        let root = vstack((
            empty().frame(140.0, 60.0).background_color(Color::hex(0xF2F3F7)).corner_radius(9.0),
            empty().frame(140.0, 90.0).background_color(Color::hex(0x3B82F6)).corner_radius(40.0),
        ));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 180.0, height: 200.0 }, 2, Color::hex(0x18181D));
        assert_close(&gpu, &cpu, 2, "rounded fills");
    }

    #[test]
    fn stroke_ring_never_double_blends() {
        if !device_present() {
            return;
        }
        // a TRANSLUCENT border is the double-blend trap: straight bars
        // must meet without overlap, the rounded ring must follow the
        // curve — one blend per pixel on both backends
        let veil = Color::rgba(0, 0, 0, 90);
        let root = vstack((
            empty().frame(120.0, 40.0).border(veil, 1.0),
            empty().frame(120.0, 40.0).border(veil, 3.0).corner_radius(12.0),
            empty()
                .frame(120.0, 40.0)
                .background_color(Color::hex(0xDDE1E9))
                .border(Color::hex(0x3B82F6), 2.0)
                .corner_radius(9.0),
        ));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 160.0, height: 160.0 }, 2, Color::CANVAS);
        assert_close(&gpu, &cpu, 2, "stroke rings");
    }

    #[test]
    fn the_gradients_ramp_the_same_on_both_backends() {
        if !device_present() {
            return;
        }
        use bunny_ui::layout::{Gradient, UnitPoint};
        // rings off-centre with a rounded box, a ramp across a wide
        // one, and a glow that fades to its own color with no alpha —
        // the three shapes a product paints
        let violet = Color::hex(0x8B5CF6);
        let root = vstack((
            empty()
                .frame(120.0, 60.0)
                .background_gradient(
                    Gradient::radial(Color::hex(0xE879F9), Color::hex(0x1E1B4B))
                        .center(UnitPoint::TOP_LEADING)
                        .radius(4.0, 90.0),
                )
                .corner_radius(12.0),
            empty().frame(120.0, 40.0).background_gradient(Gradient::linear(
                Color::hex(0x0EA5E9),
                Color::hex(0x14532D),
            )),
            empty().frame(120.0, 60.0).background_gradient(
                Gradient::radial(violet, violet.fade()).center(UnitPoint::BOTTOM),
            ),
        ));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 160.0, height: 180.0 }, 2, Color::CANVAS);
        assert_close(&gpu, &cpu, 2, "gradients");
    }

    #[test]
    fn the_elliptical_wash_ramps_the_same_on_both_backends() {
        if !device_present() {
            return;
        }
        use bunny_ui::layout::{Gradient, UnitPoint};
        // an elliptical wash across a wide bar — the aspect rides the
        // corner slot, so the kinds diverge from the circle and the
        // two rasterizers must agree
        let root = empty().frame(280.0, 90.0).background_gradient(
            Gradient::radial(Color::hex(0x7C5CFF), Color::hex(0x7C5CFF).fade())
                .center(UnitPoint::TOP_LEADING)
                .radius(98.0, 140.0)
                .aspect(65.0 / 140.0),
        );
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 300.0, height: 110.0 }, 2, Color::CANVAS);
        assert_close(&gpu, &cpu, 2, "elliptical wash");
    }

    #[test]
    fn shadow_quadratic_falloff_matches() {
        if !device_present() {
            return;
        }
        // the halo and the notch behind the rounded corner — quadratic
        // falloff, strictly outside the shape
        let root = empty()
            .frame(120.0, 80.0)
            .background_color(Color::hex(0xFFFFFF))
            .corner_radius(9.0)
            .shadow(24.0)
            .padding_length(40.0);
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 200.0, height: 160.0 }, 2, Color::CANVAS);
        assert_close(&gpu, &cpu, 2, "shadow halo");
    }

    #[test]
    fn degenerate_thin_rects_survive() {
        if !device_present() {
            return;
        }
        // hairlines and borders thicker than the box: the clamps must
        // agree on both backends, no panic, no stray ink
        let root = vstack((
            empty().frame(100.0, 1.0).background_color(Color::hex(0x18181D)),
            empty().frame(1.0, 40.0).background_color(Color::hex(0x18181D)),
            empty().frame(60.0, 10.0).border(Color::hex(0x3B82F6), 20.0),
            empty().frame(40.0, 12.0).background_color(Color::hex(0xDDE1E9)).corner_radius(30.0),
        ));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 140.0, height: 120.0 }, 2, Color::CANVAS);
        assert_close(&gpu, &cpu, 2, "degenerate rects");
    }

    #[test]
    fn text_runs_match_byte_for_byte_with_the_pixel_font() {
        if !device_present() {
            return;
        }
        // the pixel font has no anti-aliasing: alpha is 0 or 255, so the
        // sprite path must be EXACT — any drift is a texel-address bug
        let root = vstack((
            text("the quick brown bunny"),
            text("jumps over the lazy dog").foreground_color(Color::hex(0x3B82F6)),
        ))
        .padding_length(8.0)
        .background_color(Color::hex(0xFFFFFF));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 240.0, height: 80.0 }, 2, Color::CANVAS);
        assert!(
            gpu == cpu,
            "pixel-font text diverged (max channel delta {})",
            max_channel_delta(&gpu, &cpu)
        );
    }

    #[test]
    fn freetype_runs_match_within_tolerance() {
        if !device_present() {
            return;
        }
        // the real engine, SAME instance on both sides: identical run
        // rasters in, so only blend rounding may differ
        use std::rc::Rc;
        let engine = crate::text::FreeTypeEngine::new();
        let logical = Size { width: 260.0, height: 100.0 };
        let scale = 2usize;
        let physical = (520, 200);
        let runtime = Runtime::new().text_engine(Rc::new(crate::text::FreeTypeEngine::new()));
        let root = vstack((
            text("Fjord glyphs vex quick waltz"),
            text("bunny_ui presents by vulkan").foreground_color(Color::hex(0x3B82F6)),
        ))
        .padding_length(10.0)
        .background_color(Color::hex(0xFFFFFF))
        .corner_radius(9.0);
        let display = runtime.display_frame(&root, logical);
        let cpu = rasterize_with(
            &display,
            physical.0,
            physical.1,
            scale,
            Color::CANVAS,
            &engine,
            &RawImages::default(),
        )
        .to_rgba_bytes();
        let mut gpu = OffscreenVk::new(physical.0, physical.1).expect("offscreen gpu");
        gpu.present_wait(&display, scale, Color::CANVAS, &engine, &RawImages::default());
        assert_close(&gpu.read_rgba(), &cpu, 2, "freetype runs");
    }

    #[test]
    fn wide_run_chunks_are_seamless() {
        if !device_present() {
            return;
        }
        // 80 chars × 8 px × scale 2 = 1280 device px — wider than one
        // chunk, so the run splits; texel copies are 1:1 and a seam
        // would be a byte difference, not a smudge
        let long = "abcdefghij".repeat(8);
        let root = text(long).padding_length(4.0).background_color(Color::hex(0xFFFFFF));
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 700.0, height: 40.0 }, 2, Color::CANVAS);
        assert!(
            gpu == cpu,
            "chunked run diverged (max channel delta {})",
            max_channel_delta(&gpu, &cpu)
        );
    }

    #[test]
    fn a_warm_atlas_reuses_its_tiles() {
        if !device_present() {
            return;
        }
        let root = vstack((text("warm"), text("frame")));
        let logical = Size { width: 120.0, height: 60.0 };
        let runtime = Runtime::new();
        let display = runtime.display_frame(&root, logical);
        let mut gpu = OffscreenVk::new(240, 120).expect("offscreen gpu");
        gpu.present_wait(&display, 2, Color::CANVAS, &PixelFont, &RawImages::default());
        let first = gpu.atlas_footprint();
        assert!(first.0 > 0, "the frame rasterized runs into the atlas");
        gpu.present_wait(&display, 2, Color::CANVAS, &PixelFont, &RawImages::default());
        let second = gpu.atlas_footprint();
        assert_eq!(first, second, "an identical frame must not mint new tiles");
    }

    #[test]
    fn empty_clip_kills_the_quad() {
        if !device_present() {
            return;
        }
        // a zero-height frame degenerates the clip: nothing under it may
        // paint, on either backend
        let rows: Vec<usize> = vec![1, 2, 3];
        let root = list(rows, |row| row.to_string(), |_| {
            empty().frame(100.0, 20.0).background_color(Color::hex(0xFF0000))
        })
        .frame(120.0, 0.0);
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 140.0, height: 60.0 }, 2, Color::CANVAS);
        assert!(gpu == cpu, "empty-clip scene diverged");
        for pixel in gpu.chunks_exact(4) {
            assert_eq!(pixel, [0xF2, 0xF3, 0xF7, 0xFF], "a clipped-out row leaked ink");
        }
    }

    // MARK: - Images

    /// A 32×32 deterministic gradient in the house raw format.
    fn gradient_source(key: u64) -> ImageSource {
        let mut rgba = Vec::with_capacity(32 * 32 * 4);
        for y in 0..32u32 {
            for x in 0..32u32 {
                rgba.extend_from_slice(&[(x * 8) as u8, (y * 8) as u8, 128, 255]);
            }
        }
        ImageSource::bytes_keyed(key, RawImages::encode(32, 32, &rgba))
    }

    fn image_scene() -> impl View {
        let icon = gradient_source(1);
        // small rides the atlas; 300pt at scale 2 is 600×600 physical —
        // over the area threshold, a dedicated texture
        vstack((
            image(icon.clone()).resizable().frame(24.0, 24.0),
            image(gradient_source(2)).resizable().frame(300.0, 300.0),
            image(ImageSource::from_bytes(&b"junk"[..])).resizable().frame(40.0, 40.0),
            image(icon).resizable().aspect_ratio(ContentMode::Fill).frame(60.0, 20.0),
        ))
    }

    #[test]
    fn images_match_byte_for_byte() {
        if !device_present() {
            return;
        }
        // 1:1 texel blits on both sides, no anti-aliasing anywhere —
        // the engines hand both pipelines the SAME resampled bytes
        let (gpu, cpu) =
            scene_bytes(&image_scene(), Size { width: 320.0, height: 400.0 }, 2, Color::CANVAS);
        assert!(
            gpu == cpu,
            "image scene diverged (max channel delta {})",
            max_channel_delta(&gpu, &cpu)
        );
    }

    #[test]
    fn a_warm_image_frame_reuses_every_upload() {
        if !device_present() {
            return;
        }
        let runtime = Runtime::new();
        let root = image_scene();
        let display = runtime.display_frame(&root, Size { width: 320.0, height: 400.0 });
        let engine = RawImages::default();
        let mut gpu = OffscreenVk::new(640, 800).expect("offscreen gpu");
        gpu.present_wait(&display, 2, Color::CANVAS, &PixelFont, &engine);
        let first = gpu.atlas_footprint();
        assert!(first.0 >= 3, "atlas icon + cover + dedicated photo: {first:?}");
        gpu.present_wait(&display, 2, Color::CANVAS, &PixelFont, &engine);
        assert_eq!(first, gpu.atlas_footprint(), "a warm frame re-uploads nothing");
    }

    // MARK: - Icons

    const MARK_PATH: &[bunny_ui::icon::Verb] = &[
        bunny_ui::icon::Verb::Move(4.0, 12.0),
        bunny_ui::icon::Verb::Line(10.0, 18.0),
        bunny_ui::icon::Verb::Line(20.0, 6.0),
    ];
    const DISC_PATH: &[bunny_ui::icon::Verb] = &[
        bunny_ui::icon::Verb::Move(12.0, 2.0),
        bunny_ui::icon::Verb::Cubic(17.5, 2.0, 22.0, 6.5, 22.0, 12.0),
        bunny_ui::icon::Verb::Cubic(22.0, 17.5, 17.5, 22.0, 12.0, 22.0),
        bunny_ui::icon::Verb::Cubic(6.5, 22.0, 2.0, 17.5, 2.0, 12.0),
        bunny_ui::icon::Verb::Cubic(2.0, 6.5, 6.5, 2.0, 12.0, 2.0),
        bunny_ui::icon::Verb::Close,
    ];
    const MARK_GLYPH: bunny_ui::icon::Glyph = bunny_ui::icon::Glyph {
        draws: &[bunny_ui::icon::Draw {
            paint: bunny_ui::icon::Paint::Stroke { width: 2.0 },
            path: MARK_PATH,
            tint: None,
        }],
    };
    const DISC_GLYPH: bunny_ui::icon::Glyph = bunny_ui::icon::Glyph {
        draws: &[bunny_ui::icon::Draw {
            paint: bunny_ui::icon::Paint::Fill(bunny_ui::icon::Rule::NonZero),
            path: DISC_PATH,
            tint: None,
        }],
    };
    const MARK: bunny_ui::icon::Symbol = bunny_ui::icon::Symbol::new("test.mark", &MARK_GLYPH);
    const DISC: bunny_ui::icon::Symbol = bunny_ui::icon::Symbol::new("test.disc", &DISC_GLYPH);

    fn icon_scene() -> impl View {
        // two glyphs, three tints, two sizes — natural beside text,
        // exact through the frame idiom
        vstack((
            icon(MARK),
            icon(MARK).foreground_color(Color::hex(0x3366AA)),
            icon(DISC).foreground_color(Color::hex(0xAA3322)),
            icon(DISC).resizable().frame(32.0, 32.0),
            text("beside").foreground_color(Color::hex(0x222222)),
        ))
        .spacing(4.0)
    }

    #[test]
    fn icons_match_the_cpu_within_one_blend_step() {
        if !device_present() {
            return;
        }
        // the house rasterizes the glyph once and both pipelines blit
        // those same bytes — every interior texel (alpha 255) replaces
        // the destination and is EXACT. Only the anti-aliased edge
        // blends a partial alpha, and there the raster unit's rounding
        // may sit one step from the CPU's integer div255 — the same
        // documented deviation the windows port answers within one.
        let (gpu, cpu) =
            scene_bytes(&icon_scene(), Size { width: 120.0, height: 160.0 }, 2, Color::CANVAS);
        let delta = max_channel_delta(&gpu, &cpu);
        assert!(delta <= 1, "icon edges drifted by {delta} (allowed 1)");
    }

    #[test]
    fn a_warm_icon_frame_reuses_every_upload() {
        if !device_present() {
            return;
        }
        let runtime = Runtime::new();
        let root = icon_scene();
        let display = runtime.display_frame(&root, Size { width: 120.0, height: 160.0 });
        let engine = RawImages::default();
        let mut gpu = OffscreenVk::new(240, 320).expect("offscreen gpu");
        gpu.present_wait(&display, 2, Color::CANVAS, &PixelFont, &engine);
        let first = gpu.atlas_footprint();
        assert!(first.0 >= 4, "four tinted tiles at least: {first:?}");
        gpu.present_wait(&display, 2, Color::CANVAS, &PixelFont, &engine);
        assert_eq!(first, gpu.atlas_footprint(), "the tinted keys cache, never thrash");
    }

    #[test]
    fn an_ultra_wide_image_never_touches_the_shelf() {
        if !device_present() {
            return;
        }
        // 2100pt at scale 2 = 4200 physical — wider than the atlas can
        // ever grow; the dedicated path must carry it without a single
        // reset-retry (the livelock shape)
        let root = image(gradient_source(3)).resizable().frame(2100.0, 60.0);
        let (gpu, cpu) =
            scene_bytes(&root, Size { width: 400.0, height: 100.0 }, 2, Color::CANVAS);
        assert!(gpu == cpu, "ultra-wide image diverged");
        assert!(
            gpu.chunks_exact(4).any(|pixel| pixel[..3] != [0xF2, 0xF3, 0xF7]),
            "the image painted through the dedicated texture"
        );
    }

    /// The committed SPIR-V must be the committed GLSL, compiled: when
    /// the box has the compiler, re-bake and byte-compare — a shader
    /// edited without re-baking fails here on any dev box.
    #[test]
    fn the_committed_spirv_matches_its_source() {
        let compiler = std::process::Command::new("glslangValidator")
            .arg("--version")
            .output();
        if compiler.is_err() {
            eprintln!("no glslangValidator - skipping the drift gate");
            return;
        }
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/shaders");
        for stage in ["rect.vert", "rect.frag", "sprite.vert", "sprite.frag", "mask.vert", "mask.frag"] {
            let out = std::env::temp_dir().join(format!("bunny-drift-{stage}.spv"));
            let status = std::process::Command::new("glslangValidator")
                .arg("-V")
                .arg(base.join(stage))
                .arg("-o")
                .arg(&out)
                .status()
                .expect("the compiler runs");
            assert!(status.success(), "{stage} no longer compiles");
            let fresh = std::fs::read(&out).expect("the baked file exists");
            let committed = std::fs::read(base.join(format!("{stage}.spv"))).expect("blob");
            assert_eq!(fresh, committed, "{stage}.spv drifted from its source - re-bake it");
            let _ = std::fs::remove_file(out);
        }
    }

    #[test]
    fn the_push_range_holds_its_layout() {
        assert_eq!(std::mem::size_of::<Push>(), 56);
        assert_eq!(std::mem::offset_of!(Push, quad), 16);
        assert_eq!(std::mem::offset_of!(Push, round_radii), 32);
        assert_eq!(std::mem::offset_of!(Push, viewport), 48);
    }
}


