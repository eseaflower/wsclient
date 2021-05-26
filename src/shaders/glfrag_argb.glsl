
#version 450

precision highp float;
precision highp sampler2D;

in vec2 image_coord;
out vec4 f_color;

layout(binding=0) uniform sampler2D image_texture;

void main() {
    f_color = texture(image_texture, image_coord);

    // Check if the color is close to a grey scale, if so
    // clamp it to grey. We need accurate grey representation.
    float color_mean = (f_color.r + f_color.g + f_color.b) / 3.0;
    ivec3 quantized_diff = ivec3(abs(f_color - color_mean) *  255.0);
    int max_component_diff = max(max(quantized_diff.r, quantized_diff.g), quantized_diff.b);
    if (max_component_diff <= 2) {
        // This is "greyish" clamp it!
        f_color = vec4(color_mean, color_mean, color_mean, f_color.a);
    }
}