// Copyright 2026 The Android Open Source Project
// SPDX-License-Identifier: Apache-2.0

#include "vm_display_access.h"

#include <cstdio>

int main() {
    constexpr uid_t owner = 10387;
    VmDisplayAccess original(owner);
    VmDisplayAccess other(owner + 1);
    if (!original.permits(owner) || original.permits(owner + 1) ||
        original.permits(owner + 100000) || original.permits(0) || other.permits(owner)) {
        std::fputs("FAIL: display handle crossed a UID or Android-user boundary\n", stderr);
        return 1;
    }
    original.revoke();
    VmDisplayAccess replacement(owner);
    if (original.permits(owner) || original.permits(owner + 1) || !replacement.permits(owner)) {
        std::fputs("FAIL: a retired display remained usable or revoked its replacement\n", stderr);
        return 1;
    }
    std::puts("PASS: owner-only, full Android UID, transferred-handle refusal, independent revocation");
    return 0;
}
