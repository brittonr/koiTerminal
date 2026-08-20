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

internal data class DirectForwardingConfig(
    val serviceType: String,
    val serviceName: String,
    val ports: IntArray,
)

/** JSON model for direct forwarding metadata in a custom image configuration. */
internal data class DirectForwardingJson(
    private val service_type: String?,
    private val service_name: String?,
    private val ports: IntArray?,
) {
    fun toConfig(): DirectForwardingConfig {
        val serviceType = service_type.orEmpty()
        val serviceName = service_name.orEmpty()
        require(SERVICE_TYPE_PATTERN.matches(serviceType)) {
            "direct_forwarding.service_type must be a valid TCP DNS-SD service type"
        }
        require(serviceName.isNotBlank() && serviceName.length <= MAX_SERVICE_NAME_LENGTH) {
            "direct_forwarding.service_name must contain 1 to $MAX_SERVICE_NAME_LENGTH characters"
        }
        val configuredPorts = ports ?: intArrayOf()
        require(configuredPorts.isNotEmpty()) { "direct_forwarding.ports must not be empty" }
        require(configuredPorts.size <= MAX_FORWARDED_PORTS) {
            "direct_forwarding.ports supports at most $MAX_FORWARDED_PORTS entries"
        }
        configuredPorts.forEach { port ->
            require(port in MIN_FORWARDED_PORT..MAX_FORWARDED_PORT) {
                "direct_forwarding.ports contains invalid or privileged port: $port"
            }
        }
        return DirectForwardingConfig(
            serviceType,
            serviceName,
            configuredPorts.distinct().sorted().toIntArray(),
        )
    }

    companion object {
        private const val MIN_FORWARDED_PORT = 1024
        private const val MAX_FORWARDED_PORT = 65535
        private const val MAX_FORWARDED_PORTS = 16
        private const val MAX_SERVICE_NAME_LENGTH = 63
        private val SERVICE_TYPE_PATTERN = Regex("^_[A-Za-z0-9-]{1,15}\\._tcp$")
    }
}
