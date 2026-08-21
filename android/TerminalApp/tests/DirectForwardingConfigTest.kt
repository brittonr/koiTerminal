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

import com.google.gson.JsonParser
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test

class DirectForwardingConfigTest {
    private fun record(json: String) = JsonParser.parseString(json).asJsonObject

    @Test
    fun validConfigSortsAndDeduplicatesPorts() {
        val config =
            DirectForwardingConfig.fromJson(
                record(
                    """{"service_type":"_ssh._tcp","service_name":"onix-ssh","ports":[2222,8080,2222]}"""
                )
            )

        assertArrayEquals(intArrayOf(2222, 8080), config!!.ports)
    }

    @Test
    fun nullRecordYieldsNoForwarding() {
        assertNull(DirectForwardingConfig.fromJson(null))
    }

    @Test
    fun invalidServiceAndPortShapesAreRejected() {
        assertThrows(IllegalArgumentException::class.java) {
            DirectForwardingConfig.fromJson(
                record("""{"service_type":"ssh","service_name":"onix-ssh","ports":[2222]}""")
            )
        }
        assertThrows(IllegalArgumentException::class.java) {
            DirectForwardingConfig.fromJson(
                record("""{"service_type":"_ssh._tcp","service_name":"","ports":[2222]}""")
            )
        }
        assertThrows(IllegalArgumentException::class.java) {
            DirectForwardingConfig.fromJson(
                record("""{"service_type":"_ssh._tcp","service_name":"onix-ssh","ports":[22]}""")
            )
        }
        assertThrows(IllegalArgumentException::class.java) {
            DirectForwardingConfig.fromJson(
                record("""{"service_type":"_ssh._tcp","service_name":"onix-ssh","ports":[]}""")
            )
        }
        assertThrows(IllegalArgumentException::class.java) {
            val many = (1..17).joinToString(",") { "2222" }
            DirectForwardingConfig.fromJson(
                record("""{"service_type":"_ssh._tcp","service_name":"onix-ssh","ports":[$many]}""")
            )
        }
    }
}
