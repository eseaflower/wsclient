
#version 450

precision highp float;
precision highp sampler2D;

in vec2 image_coord;
out vec4 f_color;

layout(binding=0) uniform sampler2D image_texture;

void main() {
    float val = texture(image_texture, image_coord).g;
    f_color= vec4(val, val, val, 1.0);
}