#ifndef HANZOD_FFI_H
#define HANZOD_FFI_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

int hanzod_ffi_start(const char *data_dir, uint16_t port);
int hanzod_ffi_stop(void);
int hanzod_ffi_running(void);
char *hanzod_ffi_status_json(void);
void hanzod_ffi_free_string(char *ptr);

#ifdef __cplusplus
}
#endif

#endif
