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

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class DirectForwardingConfigTest {
    @Test
    fun validConfigSortsAndDeduplicatesPorts() {
        val config =
            DirectForwardingJson("_ssh._tcp", "onix-ssh", intArrayOf(2222, 8080, 2222)).toConfig()

        assertArrayEquals(intArrayOf(2222, 8080), config.ports)
    }

    @Test
    fun invalidServiceAndPortShapesAreRejected() {
        assertThrows(IllegalArgumentException::class.java) {
            DirectForwardingJson("ssh", "onix-ssh", intArrayOf(2222)).toConfig()
        }
        assertThrows(IllegalArgumentException::class.java) {
            DirectForwardingJson("_ssh._tcp", "", intArrayOf(2222)).toConfig()
        }
        assertThrows(IllegalArgumentException::class.java) {
            DirectForwardingJson("_ssh._tcp", "onix-ssh", intArrayOf(22)).toConfig()
        }
        assertThrows(IllegalArgumentException::class.java) {
            DirectForwardingJson("_ssh._tcp", "onix-ssh", IntArray(17) { 2222 }).toConfig()
        }
    }
}
