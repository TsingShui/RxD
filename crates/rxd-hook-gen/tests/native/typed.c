/* Independently authored #135 fixture; Apache-2.0. Never execute this sample. */
__attribute__((noinline)) int signed_scalar(int count) { return count - 3; }
__attribute__((noinline)) unsigned long long wide_scalar(unsigned long long token) { return token ^ 0x123456789abcdef0ULL; }
__attribute__((noinline)) long long signed_wide(long long delta) { return delta - 17; }
__attribute__((noinline)) const void *pointer_scalar(const void *data) { return data; }
__attribute__((noinline)) unsigned int mixed_scalar(int signed_value, unsigned char byte_value, short short_value, const void *pointer_value) { return signed_value + byte_value + short_value + (pointer_value != (void *)0); }
__attribute__((noinline)) float unsupported_float(float fraction) { return fraction; }
__attribute__((noinline)) int unsupported_variadic(int count, ...) { return count; }
/* A tiny selected function must not decompile this unrelated function. */
__attribute__((noinline)) unsigned int unrelated(unsigned int n) { unsigned int r=1; for (unsigned int i=0;i<n;i++) { r = (r*33) ^ (i+7); } return r; }
__attribute__((noinline)) int caller(int n) { return signed_scalar(n) + mixed_scalar(n, 2, -3, (void *)0); }
