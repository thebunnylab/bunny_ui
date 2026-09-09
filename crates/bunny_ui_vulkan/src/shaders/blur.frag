// One separable pass of the liquid-glass blur pyramid — nine bilinear
// taps, a seventeen-tap gaussian at sigma 2.6 texels of the
// DESTINATION, which is half the resolution of the source. That is what
// makes the downsample free: every tap already averages a 2x2
// neighbourhood.
//
// The push block is the frame's, reused field for field: `round_box`
// carries the mode and `quad` carries the step, so the pipeline layout
// this tier already has needs no second range.
#version 450

layout(push_constant) uniform Push {
    // x = the mip this pass reads; y = 1 when the source is raw scene
    // colour, which no format decodes for us
    vec4 mode;
    // x, y = 1 / destination size; z, w = the direction, (1,0) or (0,1)
    vec4 step_uv;
    vec4 round_radii;
    vec2 viewport;
} pc;

layout(set = 0, binding = 0) uniform sampler2D source;

layout(location = 0) out vec4 out_color;

const float BLUR_W[5] = float[5](0.153584, 0.256886, 0.125975, 0.034902, 0.005445);
const float BLUR_O[5] = float[5](0.0, 1.44475, 3.37341, 5.30746, 7.24824);

vec3 srgb_to_linear3(vec3 c) {
    return mix(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92,
               lessThanEqual(c, vec3(0.04045)));
}

vec4 blur_tap(vec2 uv) {
    vec4 c = textureLod(source, uv, pc.mode.x);
    // colour only: a transfer function never applies to alpha
    return pc.mode.y != 0.0 ? vec4(srgb_to_linear3(c.rgb), c.a) : c;
}

void main() {
    vec2 inv = pc.step_uv.xy;
    vec2 uv = gl_FragCoord.xy * inv;
    vec2 away_step = pc.step_uv.zw * inv;
    vec4 acc = blur_tap(uv) * BLUR_W[0];
    for (int i = 1; i < 5; i++) {
        vec2 away = away_step * BLUR_O[i];
        acc += (blur_tap(uv + away) + blur_tap(uv - away)) * BLUR_W[i];
    }
    out_color = acc;
}
