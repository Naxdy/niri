// Ported from https://github.com/nferhat/fht-compositor/blob/main/src/renderer/shaders/blur-finish.frag
//
// Implementation from pinnacle-comp/pinnacle (GPL-3.0)
// Thank you very much!
#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

#if defined(EXTERNAL)
uniform samplerExternalOES alpha_tex;
#else
uniform sampler2D alpha_tex;
#endif

uniform float alpha;
varying vec2 v_coords;

uniform vec4 corner_radius;
uniform mat3 input_to_geo;
uniform vec2 geo_size;
uniform float niri_scale;
uniform float noise;
uniform float brightness;
uniform float contrast;
uniform float saturation;
uniform float ignore_alpha;

float rounding_alpha(vec2 coords, vec2 size) {
    vec2 center;
    float radius;

    if (coords.x < corner_radius.x && coords.y < corner_radius.x) {
        radius = corner_radius.x;
        center = vec2(radius, radius);
    } else if (size.x - corner_radius.y < coords.x && coords.y < corner_radius.y) {
        radius = corner_radius.y;
        center = vec2(size.x - radius, radius);
    } else if (size.x - corner_radius.z < coords.x && size.y - corner_radius.z < coords.y) {
        radius = corner_radius.z;
        center = vec2(size.x - radius, size.y - radius);
    } else if (coords.x < corner_radius.w && size.y - corner_radius.w < coords.y) {
        radius = corner_radius.w;
        center = vec2(radius, size.y - radius);
    } else {
        return 1.0;
    }

    float dist = distance(coords, center);
    float half_px = 0.5 / niri_scale;
    return 1.0 - smoothstep(radius - half_px, radius + half_px, dist);
}

// Noise function copied from hyprland.
// I like the effect it gave, can be tweaked further
float hash(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 727.727); // wysi :wink: :wink:
    p3 += dot(p3, p3.xyz + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// Taken from https://github.com/wlrfx/scenefx/blob/main/render/fx_renderer/gles3/shaders/blur_effects.frag
mat4 brightnessMatrix() {
    float b = brightness - 1.0;
    return mat4(1, 0, 0, 0,
                0, 1, 0, 0,
                0, 0, 1, 0,
                b, b, b, 1);
}
mat4 contrastMatrix() {
    float t = (1.0 - contrast) / 2.0;
    return mat4(contrast, 0, 0, 0,
                0, contrast, 0, 0,
                0, 0, contrast, 0,
                t, t, t, 1);
}
mat4 saturationMatrix() {
    vec3 luminance = vec3(0.3086, 0.6094, 0.0820) * (1.0 - saturation);
    vec3 red = vec3(luminance.x);
    red.x += saturation;
    vec3 green = vec3(luminance.y);
    green.y += saturation;
    vec3 blue = vec3(luminance.z);
    blue.z += saturation;
    return mat4(red, 0,
                green, 0,
                blue, 0,
                0, 0, 0, 1);
}

void main() {
    if (alpha <= 0.0) {
      discard;
    }

    if (ignore_alpha > 0.0) {
      vec4 alpha_color = texture2D(alpha_tex, v_coords);
      if (alpha_color.a < ignore_alpha) {
        discard;
      }
    }

    vec3 coords_geo = input_to_geo * vec3(v_coords, 1.0);

    // Sample the texture.
    vec4 color = texture2D(tex, v_coords);
    color = brightnessMatrix() * contrastMatrix() * saturationMatrix() * color;

#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif

    if (coords_geo.x < 0.0 || 1.0 < coords_geo.x || coords_geo.y < 0.0 || 1.0 < coords_geo.y) {
        // Clip outside geometry.
        color = vec4(0.0);
    } else {
        // Apply corner rounding inside geometry.
        color = color * rounding_alpha(coords_geo.xy * geo_size, geo_size);
    }

    if (color.a <= 0.0) {
      discard;
    }

    if (noise > 0.0) {
      // Add noise fx
      // This can be used to achieve a glass look
      float noiseHash   = hash(v_coords);
      float noiseAmount = (mod(noiseHash, 1.0) - 0.5);
      color.rgb += noiseAmount * noise;
    }


    color *= alpha;

    gl_FragColor = color;
}

// vim: ft=glsl
