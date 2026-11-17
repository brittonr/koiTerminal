/* Copyright 2026 The Android Open Source Project
 * SPDX-License-Identifier: Apache-2.0
 */
package android.system.virtualizationservice;

/** Kernel-Binder display broker. This is not the privileged internal VM service. */
interface IVirtualMachineDisplayService {
    const String SERVICE_NAME = "android.system.virtualizationdisplay";
    const int ERROR_NOT_READY = 1;

    /**
     * Returns the display of the caller's registered VM, without waiting for another VM.
     * Requires MANAGE_VIRTUAL_MACHINE and USE_CUSTOM_VIRTUAL_MACHINE. The owner is checked
     * against the full kernel Binder calling UID captured when this CID was allocated.
     * Throws SecurityException for an unknown/foreign CID, or ERROR_NOT_READY if unpublished.
     * The returned service independently enforces the owner's UID on every transaction.
     */
    IBinder getDisplayService(int cid);
}
