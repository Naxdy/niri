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

uniform vec4 geo;
uniform vec2 output_size;
uniform float corner_radius;
uniform float noise;
uniform float ignore_alpha;

float rounding_alpha(vec2 coords, vec2 size, float radius) {
    vec2 center;

    if (coords.x < radius && coords.y < radius) {
        center = vec2(radius, radius);
    } else if (coords.x > size.x - radius && coords.y < radius) {
        center = vec2(size.x - radius, radius);
    } else if (coords.x > size.x - radius && coords.y > size.y - radius) {
        center = vec2(size.x - radius, size.y - radius);
    } else if (coords.x < radius && coords.y > size.y - radius) {
        center = vec2(radius, size.y - radius);
    } else {
        return 1.0;
    }

    float dist = distance(coords, center);
    float half_px = 0.5 ;
    return 1.0 - smoothstep(radius - half_px, radius + half_px, dist);
}

// Noise function copied from hyprland.
// I like the effect it gave, can be tweaked further
float hash(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 727.727); // wysi :wink: :wink:
    p3 += dot(p3, p3.xyz + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

void main() {
    vec2 texCoords;

    // Sample the texture.
    vec4 color = texture2D(tex, v_coords);

#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif
    float alphaMask = 1.0;

    if (ignore_alpha > 0.0) {
      vec4 alpha_color = texture2D(alpha_tex, v_coords);
      if (alpha_color.a < ignore_alpha) {
        alphaMask = 0.0;
      }
    }

    // This shader exists to make blur rounding correct.
    // 
    // Since we are scr-ing a texture that is the size of the output, the v_coords are always
    // relative to the output. This corresponds to gl_FragCoord.
    vec2 size = geo.zw;
    // NOTE: this is incorrect when rendering in winit, since y is inverted,
    // but on tty produces the correct result, which is all that matters
    vec2 loc = gl_FragCoord.xy - geo.xy;

    // Add noise fx
    // This can be used to achieve a glass look
    float noiseHash   = hash(loc / size);
    float noiseAmount = (mod(noiseHash, 1.0) - 0.5);

    if (alphaMask > 0.0) {
      color.rgb += noiseAmount * noise;
    }

    // Apply corner rounding inside geometry.
    if (corner_radius > 0.0) {
      color *= rounding_alpha(loc, size, corner_radius);
    }
    color *= alpha;
    color *= alphaMask;

    gl_FragColor = color;
}

// vim: ft=glsl
