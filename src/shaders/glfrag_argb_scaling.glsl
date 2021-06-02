
#version 450

precision highp float;
precision highp sampler2D;

in vec2 image_coord;
out vec4 f_color;

layout(binding=0) uniform sampler2D image_texture;


#define FIX(c) max(abs(c), 1e-5);
const float PI = 3.1415926535897932384626433832795;



const mat3 rgb_to_yuv = mat3(
    0.2126, 0.7152, 0.0722, // Column 1
    -0.114572, -0.385428, 0.5, // Column 2
    0.5, -0.454153, -0.045847 // Column 3
);
const float CHROMA_THRESHOLD = 1.5;

// from http://www.java-gaming.org/index.php?topic=35123.0
vec4 cubic(float v){
    vec4 n = vec4(1.0, 2.0, 3.0, 4.0) - v;
    vec4 s = n * n * n;
    float x = s.x;
    float y = s.y - 4.0 * s.x;
    float z = s.z - 4.0 * s.y + 6.0 * s.x;
    float w = 6.0 - x - y - z;
    return vec4(x, y, z, w) * (1.0/6.0);
}

vec4 textureBicubic(sampler2D sampler, vec2 texCoords){

   vec2 texSize = textureSize(sampler, 0);
   vec2 invTexSize = 1.0 / texSize;

   texCoords = texCoords * texSize - 0.5;


    vec2 fxy = fract(texCoords);
    texCoords -= fxy;

    vec4 xcubic = cubic(fxy.x);
    vec4 ycubic = cubic(fxy.y);

    vec4 c = texCoords.xxyy + vec2 (-0.5, +1.5).xyxy;

    vec4 s = vec4(xcubic.xz + xcubic.yw, ycubic.xz + ycubic.yw);
    vec4 offset = c + vec4 (xcubic.yw, ycubic.yw) / s;

    offset *= invTexSize.xxyy;

    vec4 sample0 = texture(sampler, offset.xz);
    vec4 sample1 = texture(sampler, offset.yz);
    vec4 sample2 = texture(sampler, offset.xw);
    vec4 sample3 = texture(sampler, offset.yw);

    float sx = s.x / (s.x + s.y);
    float sy = s.z / (s.z + s.w);

    return mix(
       mix(sample3, sample2, sx), mix(sample1, sample0, sx)
    , sy);
}



vec4 weight4lanc(float x)
{
    const float radius = 2.0;
    vec4 s = FIX(PI * vec4(1.0 + x, x, 1.0 - x, 2.0 - x));

    // // Lanczos2. Note: we normalize below, so no point in multiplying by radius.
    vec4 ret = /*radius **/ sin(s) * sin(s / radius) / (s * s);

    // // Normalize
    return ret / dot(ret, vec4(1.0));
}

vec3 pixel(sampler2D sampler, float xpos, float ypos)
{
    return texture2D(sampler, vec2(xpos, ypos)).rgb;
}

vec3 line(sampler2D sampler, float ypos, vec4 xpos, vec4 linetaps)
{
    return mat4x3(
        pixel(sampler, xpos.x, ypos),
        pixel(sampler, xpos.y, ypos),
        pixel(sampler, xpos.z, ypos),
        pixel(sampler, xpos.w, ypos)) * linetaps;
}


vec4 lanc(sampler2D sampler, vec2 texCoords)
{

    vec2 texSize = textureSize(sampler, 0);

    vec2 stepxy = 1.0 / texSize;
    vec2 pos = texCoords.xy + stepxy * 0.5;
    vec2 f = fract(pos / stepxy);

    vec2 xystart = (-1.5 - f) * stepxy + pos;
    vec4 xpos = vec4(
        xystart.x,
        xystart.x + stepxy.x,
        xystart.x + stepxy.x * 2.0,
        xystart.x + stepxy.x * 3.0);

    vec4 linetaps   = weight4lanc(f.x);
    vec4 columntaps = weight4lanc(f.y);

    vec3 res = mat4x3(
        line(sampler, xystart.y                 , xpos, linetaps),
        line(sampler, xystart.y + stepxy.y      , xpos, linetaps),
        line(sampler, xystart.y + stepxy.y * 2.0, xpos, linetaps),
        line(sampler, xystart.y + stepxy.y * 3.0, xpos, linetaps)) * columntaps;

    // gl_FragColor.a = 1.0;
    return vec4(res, 1.0);
}


float weightbisharp(float x)
{
    float ax = abs(x);
    const float B = 0.1;
    const float C = 0.5;

    if (ax < 1.0) {
    return (
        pow(x, 2.0) * (
        (12.0 - 9.0 * B - 6.0 * C) * ax +
        (-18.0 + 12.0 * B + 6.0 * C)
        ) +
        (6.0 - 2.0 * B)
    ) / 6.0;

    } else if ((ax >= 1.0) && (ax < 2.0)) {
    return (
        pow(x, 2.0) * (
        (-B - 6.0 * C) * ax +
        (6.0 * B + 30.0 * C)
        ) +
        (-12.0 * B - 48.0 * C) * ax +
        (8.0 * B + 24.0 * C)
    ) / 6.0;

    } else {
    return 0.0;
    }
}
float weightbisharper(float x)
	{
	    float ax = abs(x);
	    // Sharper version.
	    // May look better in some cases.
	    const float B = 0.0;
	    const float C = 0.75;

	    if (ax < 1.0) {
		return (
		    pow(x, 2.0) * (
			(12.0 - 9.0 * B - 6.0 * C) * ax +
			(-18.0 + 12.0 * B + 6.0 * C)
		    ) +
		    (6.0 - 2.0 * B)
		) / 6.0;

	    } else if ((ax >= 1.0) && (ax < 2.0)) {
		return (
		    pow(x, 2.0) * (
			(-B - 6.0 * C) * ax +
			(6.0 * B + 30.0 * C)
		    ) +
		    (-12.0 * B - 48.0 * C) * ax +
		    (8.0 * B + 24.0 * C)
		) / 6.0;

	    } else {
		return 0.0;
	    }
	}

vec4 weight4bisharp(float x)
{
    return vec4(
    weightbisharp(x + 1.0),
    weightbisharp(x),
    weightbisharp(1.0 - x),
    weightbisharp(2.0 - x));
}

vec4 weight4bisharper(float x)
{
    return vec4(
    weightbisharper(x + 1.0),
    weightbisharper(x),
    weightbisharper(1.0 - x),
    weightbisharper(2.0 - x));
}
vec4 bisharp(sampler2D sampler, vec2 texCoords) {
        vec2 texSize = textureSize(sampler, 0);

    vec2 stepxy = 1.0 / texSize;
    vec2 pos = texCoords.xy + stepxy * 0.5;
    vec2 f = fract(pos / stepxy);

    vec2 xystart = (-1.5 - f) * stepxy + pos;
    vec4 xpos = vec4(
        xystart.x,
        xystart.x + stepxy.x,
        xystart.x + stepxy.x * 2.0,
        xystart.x + stepxy.x * 3.0);

    vec4 linetaps   = weight4bisharper(f.x);
    vec4 columntaps = weight4bisharper(f.y);

    linetaps /=
		linetaps.r +
		linetaps.g +
		linetaps.b +
		linetaps.a;
    columntaps /=
		columntaps.r +
		columntaps.g +
		columntaps.b +
		columntaps.a;


    vec3 res = mat4x3(
        line(sampler, xystart.y                 , xpos, linetaps),
        line(sampler, xystart.y + stepxy.y      , xpos, linetaps),
        line(sampler, xystart.y + stepxy.y * 2.0, xpos, linetaps),
        line(sampler, xystart.y + stepxy.y * 3.0, xpos, linetaps)) * columntaps;

    // gl_FragColor.a = 1.0;
    return vec4(res, 1.0);

}


void main() {
    // f_color = texture(image_texture, image_coord);
    // f_color = textureBicubic(image_texture, image_coord);
    f_color = lanc(image_texture, image_coord);
    // f_color = bisharp(image_texture, image_coord);

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