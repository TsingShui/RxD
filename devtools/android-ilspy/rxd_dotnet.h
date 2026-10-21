#ifndef RXD_DOTNET_ANDROID_H
#define RXD_DOTNET_ANDROID_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Input is immutable PE bytes, never executed. Tokens are original metadata tokens.
 * Returned UTF-8 JSON is not NUL terminated. Release only with rxd_free. */
int32_t rxd_set_reference_directory(const uint8_t *utf8_path, int32_t length);
uint8_t *rxd_decompile_method(const uint8_t *module, int32_t module_length,
                              int32_t method_token, int32_t *output_length);
uint8_t *rxd_decompile_class(const uint8_t *module, int32_t module_length,
                             int32_t type_token, int32_t *output_length);
void rxd_free(uint8_t *output);
#ifdef __cplusplus
}
#endif
#endif
