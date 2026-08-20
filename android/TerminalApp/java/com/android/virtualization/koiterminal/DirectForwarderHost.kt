/*
 * Copyright (C) 2026 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */
package com.android.virtualization.koiterminal

import android.content.Context
import android.util.Log
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Exposes explicit guest TCP ports on Android loopback without a guest agent or virtiofs.
 *
 * The native forwarder owns the listening sockets and connects each accepted session to the same
 * port at [guestAddress]. Call [stop] with the VM lifecycle so no listener survives its guest.
 */
internal class DirectForwarderHost(
    context: Context,
    private val guestAddress: String,
    ports: IntArray,
) {
    private val forwardedPorts = ports.clone()
    private val started = AtomicBoolean(false)
    private val executor =
        Executors.newSingleThreadExecutor(TerminalThreadFactory(context.applicationContext))

    fun start() {
        check(started.compareAndSet(false, true)) { "direct forwarder host already started" }
        executor.execute {
            Log.i(
                TAG,
                "Starting direct forwarding to $guestAddress for ${forwardedPorts.contentToString()}",
            )
            try {
                run(guestAddress, forwardedPorts)
            } finally {
                Log.i(TAG, "Direct forwarding stopped")
                started.set(false)
            }
        }
    }

    fun stop() {
        if (started.get()) {
            shutdown()
        }
        executor.shutdown()
    }

    companion object {
        private const val TAG = "VmTerminalApp.DirectForwarderHost"

        init {
            System.loadLibrary("forwarder_host_jni")
        }

        @JvmStatic private external fun run(guestAddress: String, ports: IntArray)

        @JvmStatic private external fun shutdown()
    }
}
