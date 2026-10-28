/* Test-only dyld interposer for deterministic macOS updater rollback faults. */
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static char backup_path[PATH_MAX];
static int has_backup;

static int exact_target(const char *path) {
  const char *target = getenv("WENLAN_UPDATER_FAULT_TARGET");
  return target != NULL && path != NULL && strcmp(path, target) == 0;
}

static int staged_path(const char *path) {
  const char *base;
  if (path == NULL) return 0;
  base = strrchr(path, '/');
  base = base == NULL ? path : base + 1;
  return strncmp(base, "tauri_updated_app", 17) == 0;
}

static int armed(void) {
  const char *marker = getenv("WENLAN_UPDATER_FAULT_ARM");
  return marker != NULL && access(marker, F_OK) == 0;
}

static void record(const char *event, const char *from, const char *to,
                   int result, int error) {
  const char *path = getenv("WENLAN_UPDATER_FAULT_LOG");
  int saved_errno = errno;
  FILE *file;
  if (path == NULL) {
    errno = saved_errno;
    return;
  }
  file = fopen(path, "a");
  if (file == NULL) {
    errno = saved_errno;
    return;
  }
  fprintf(file, "%s from=%s to=%s result=%d errno=%d\n", event,
          from == NULL ? "(null)" : from, to == NULL ? "(null)" : to,
          result, error);
  fclose(file);
  errno = saved_errno;
}

static int intercepted_rename(const char *from, const char *to) {
  int result;
  if (armed() && exact_target(from) && !exact_target(to)) {
    result = rename(from, to);
    record("rename_old_to_backup", from, to, result,
           result == 0 ? 0 : errno);
    if (result == 0 && snprintf(backup_path, sizeof(backup_path), "%s", to) > 0)
      has_backup = 1;
    return result;
  }
  if (has_backup && exact_target(to) && strcmp(from, backup_path) == 0) {
    const char *marker = getenv("WENLAN_UPDATER_FAULT_RESTORE_ARM");
    if (marker != NULL && unlink(marker) == 0) {
      record("rename_restore_eio", from, to, -1, EIO);
      errno = EIO;
      return -1;
    }
    result = rename(from, to);
    record("rename_restore_old_app", from, to, result,
           result == 0 ? 0 : errno);
    return result;
  }
  if (armed() && staged_path(from) && exact_target(to)) {
    const char *marker = getenv("WENLAN_UPDATER_FAULT_ARM");
    if (marker != NULL && unlink(marker) == 0) {
      record("rename_replacement_eio", from, to, -1, EIO);
      errno = EIO;
      return -1;
    }
  }
  return rename(from, to);
}

static int intercepted_renamex_np(const char *from, const char *to,
                                  unsigned int flags) {
  int result;
  if (armed() && ((exact_target(from) && staged_path(to)) ||
                  (staged_path(from) && exact_target(to)))) {
    record("renamex_swap_enotsup", from, to, -1, ENOTSUP);
    errno = ENOTSUP;
    return -1;
  }
  result = renamex_np(from, to, flags);
  if (exact_target(to) && staged_path(from)) {
    record("renamex_swap_actual", from, to, result, result == 0 ? 0 : errno);
  }
  return result;
}

__attribute__((used)) static struct {
  const void *replacement;
  const void *replacee;
} interpose_rename __attribute__((section("__DATA,__interpose"))) = {
    (const void *)intercepted_rename, (const void *)rename};

__attribute__((used)) static struct {
  const void *replacement;
  const void *replacee;
} interpose_renamex_np __attribute__((section("__DATA,__interpose"))) = {
    (const void *)intercepted_renamex_np, (const void *)renamex_np};
