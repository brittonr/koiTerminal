// Copyright 2026 The Android Open Source Project
// SPDX-License-Identifier: Apache-2.0

#pragma once

#include <sys/types.h>

// DisplayService serializes this state with its transaction/lifecycle mutex.
class VmDisplayAccess {
public:
    explicit VmDisplayAccess(uid_t owner) : mOwner(owner) {}

    bool permits(uid_t caller) const { return mActive && caller == mOwner; }
    void revoke() { mActive = false; }

private:
    const uid_t mOwner;
    bool mActive = true;
};
