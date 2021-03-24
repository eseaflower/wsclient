
#version 450

precision highp float;
precision highp sampler2D;

in vec2 image_coord;
out vec4 f_color;

layout(binding=0) uniform sampler2D image_texture;

void main() {
    f_color = texture(image_texture, image_coord);
    // f_color = vec4(0.0, 1.0, 0.0, 1.0);
}