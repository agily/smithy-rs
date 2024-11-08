/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

package software.amazon.smithy.rust.codegen.server.smithy.protocols

import software.amazon.smithy.rust.codegen.core.smithy.generators.http.HttpBindingCustomization
import software.amazon.smithy.rust.codegen.core.smithy.generators.protocol.ProtocolSupport
import software.amazon.smithy.rust.codegen.core.smithy.protocols.HttpBindingResolver
import software.amazon.smithy.rust.codegen.core.smithy.protocols.Protocol
import software.amazon.smithy.rust.codegen.core.smithy.protocols.ProtocolGeneratorFactory
import software.amazon.smithy.rust.codegen.core.smithy.protocols.parse.JsonParserCustomization
import software.amazon.smithy.rust.codegen.core.smithy.protocols.restJsonFieldName
import software.amazon.smithy.rust.codegen.core.smithy.protocols.serialize.JsonSerializerGenerator
import software.amazon.smithy.rust.codegen.core.smithy.protocols.serialize.StructuredDataSerializerGenerator
import software.amazon.smithy.rust.codegen.server.smithy.ServerCodegenContext
import software.amazon.smithy.rust.codegen.server.smithy.customizations.BeforeIteratingOverMapOrCollectionJsonCustomization
import software.amazon.smithy.rust.codegen.server.smithy.customizations.BeforeSerializingMemberJsonCustomization
import software.amazon.smithy.rust.codegen.server.smithy.generators.protocol.ServerEc2QueryProtocol
import software.amazon.smithy.rust.codegen.server.smithy.generators.protocol.ServerRestJsonProtocol

/**
 * RestJson1 server-side protocol factory. This factory creates the [ServerHttpProtocolGenerator]
 * with RestJson1 specific configurations.
 */
class ServerEc2QueryFactory(
    private val additionalParserCustomizations: List<JsonParserCustomization> = listOf(),
    private val additionalServerHttpBoundProtocolCustomizations: List<ServerHttpBoundProtocolCustomization> = listOf(),
    private val additionalHttpBindingCustomizations: List<HttpBindingCustomization> = listOf(),
) : ProtocolGeneratorFactory<ServerHttpBoundProtocolGenerator, ServerCodegenContext> {
    override fun protocol(codegenContext: ServerCodegenContext): Protocol =
        ServerEc2QueryProtocol(codegenContext)

    override fun buildProtocolGenerator(codegenContext: ServerCodegenContext): ServerHttpBoundProtocolGenerator =
        ServerHttpBoundProtocolGenerator(
            codegenContext,
            ServerEc2QueryProtocol(
                codegenContext,
            ),
            additionalServerHttpBoundProtocolCustomizations,
            additionalHttpBindingCustomizations,
        )

    override fun support(): ProtocolSupport {
        return ProtocolSupport(
            // Client support
            requestSerialization = false,
            requestBodySerialization = false,
            responseDeserialization = false,
            errorDeserialization = false,
            // Server support
            requestDeserialization = true,
            requestBodyDeserialization = true,
            responseSerialization = true,
            errorSerialization = true,
        )
    }
}