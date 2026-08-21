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

import com.google.gson.JsonArray
import com.google.gson.JsonObject

internal data class DirectForwardingConfig(
    val serviceType: String,
    val serviceName: String,
    val ports: IntArray,
) {
    internal companion object {
        private const val MIN_FORWARDED_PORT = 1024
        private const val MAX_FORWARDED_PORT = 65535
        private const val MAX_FORWARDED_PORTS = 16
        private const val MAX_SERVICE_NAME_LENGTH = 63
        private val SERVICE_TYPE_PATTERN = Regex("^_[A-Za-z0-9-]{1,15}\\._tcp$")

        /**
         * Builds a forwarding config from the `direct_forwarding` JSON record.
         *
         * This is parsed through the Gson tree instead of reflective data-class binding so release
         * optimization of the app cannot break instantiation of the config type.
         */
        fun fromJson(record: JsonObject?): DirectForwardingConfig? {
            if (record == null) {
                return null
            }
            val serviceType =
                record.get("service_type")?.takeIf { it.isJsonPrimitive }?.asString.orEmpty()
            val serviceName =
                record.get("service_name")?.takeIf { it.isJsonPrimitive }?.asString.orEmpty()
            require(SERVICE_TYPE_PATTERN.matches(serviceType)) {
                "direct_forwarding.service_type must be a valid TCP DNS-SD service type"
            }
            require(serviceName.isNotBlank() && serviceName.length <= MAX_SERVICE_NAME_LENGTH) {
                "direct_forwarding.service_name must contain 1 to $MAX_SERVICE_NAME_LENGTH characters"
            }
            val portsJson: JsonArray? = record.get("ports")?.takeIf { it.isJsonArray }?.asJsonArray
            require(portsJson != null && portsJson.size() > 0) {
                "direct_forwarding.ports must not be empty"
            }
            require(portsJson.size() <= MAX_FORWARDED_PORTS) {
                "direct_forwarding.ports supports at most $MAX_FORWARDED_PORTS entries"
            }
            val configuredPorts =
                IntArray(portsJson.size()) { index ->
                    val port = portsJson.get(index).takeIf { it.isJsonPrimitive }?.asInt
                    require(port != null && port in MIN_FORWARDED_PORT..MAX_FORWARDED_PORT) {
                        "direct_forwarding.ports contains invalid or privileged port: ${portsJson.get(index)}"
                    }
                    port
                }
            return DirectForwardingConfig(
                serviceType,
                serviceName,
                configuredPorts.distinct().sorted().toIntArray(),
            )
        }
    }
}
