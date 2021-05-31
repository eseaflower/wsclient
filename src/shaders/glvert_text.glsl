
#version 450

layout(location=0) in vec2 vertex_pos;
// layout(location=0) in vec3 vertex_pos;
layout(location=1) in vec2 tex_coord;

out vec2 image_coord;

void main() {
    gl_Position = vec4(vertex_pos, 0.0,  1.0);
    // gl_Position = vec4(vertex_pos, 1.0);
    image_coord = tex_coord;
}