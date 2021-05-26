
#version 450

precision highp float;
precision highp sampler2D;

in vec2 image_coord;
out vec4 f_color;

layout(binding=0) uniform sampler2D image_texture;


const mat3 rgb_to_yuv = mat3(
    0.2126, 0.7152, 0.0722, // Column 1
    -0.114572, -0.385428, 0.5, // Column 2
    0.5, -0.454153, -0.045847 // Column 3
);
const float CHROMA_THRESHOLD = 1.5;

void main() {
    f_color = texture(image_texture, image_coord);

    // Check if the color is close to a grey scale, if so
    // clamp it to grey. We need accurate grey representation.
    // Multiply from left (to follow example in Python, otherwise transpose the matrix)
    vec3 yuv = f_color.rgb * rgb_to_yuv;
    float max_chroma = max(abs(yuv.g), abs(yuv.b)) * 255.0;
    if (max_chroma <= CHROMA_THRESHOLD) {
        // This seems like a grey, clampt it
        f_color = vec4(yuv.r, yuv.r, yuv.r, f_color.a);
    }

    // f_color = vec4(yuv.r, yuv.r, yuv.r, f_color.a);
    // float color_mean = (f_color.r + f_color.g + f_color.b) / 3.0;
    // ivec3 quantized_diff = ivec3(abs(f_color - color_mean) *  255.0);
    // int max_component_diff = max(max(quantized_diff.r, quantized_diff.g), quantized_diff.b);
    // if (max_component_diff <= 2) {
    //     // This is "greyish" clamp it!
    //     f_color = vec4(color_mean, color_mean, color_mean, f_color.a);
    // }
}