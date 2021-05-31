
#version 450

precision highp float;
precision highp sampler2D;

in vec2 image_coord;
out vec4 f_color;

layout(binding=0) uniform sampler2D image_texture;

void main() {
    float alpha = texture(image_texture, image_coord).r;
    if (alpha <= 0.0) {
        discard;
    }
    f_color = vec4(1.0, 1.0, 1.0, alpha);
}