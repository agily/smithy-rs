/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

package software.amazon.smithy.rust.codegen.core.smithy.protocols.serialize

import software.amazon.smithy.aws.traits.protocols.Ec2QueryNameTrait
import software.amazon.smithy.codegen.core.CodegenException
import software.amazon.smithy.model.shapes.BlobShape
import software.amazon.smithy.model.shapes.BooleanShape
import software.amazon.smithy.model.shapes.CollectionShape
import software.amazon.smithy.model.shapes.MapShape
import software.amazon.smithy.model.shapes.MemberShape
import software.amazon.smithy.model.shapes.NumberShape
import software.amazon.smithy.model.shapes.OperationShape
import software.amazon.smithy.model.shapes.Shape
import software.amazon.smithy.model.shapes.ShapeId
import software.amazon.smithy.model.shapes.StringShape
import software.amazon.smithy.model.shapes.StructureShape
import software.amazon.smithy.model.shapes.TimestampShape
import software.amazon.smithy.model.shapes.UnionShape
import software.amazon.smithy.model.traits.TimestampFormatTrait
import software.amazon.smithy.model.traits.XmlFlattenedTrait
import software.amazon.smithy.model.traits.XmlNameTrait
import software.amazon.smithy.model.traits.XmlNamespaceTrait
import software.amazon.smithy.rust.codegen.core.rustlang.Attribute
import software.amazon.smithy.rust.codegen.core.rustlang.RustWriter
import software.amazon.smithy.rust.codegen.core.rustlang.autoDeref
import software.amazon.smithy.rust.codegen.core.rustlang.rust
import software.amazon.smithy.rust.codegen.core.rustlang.rustBlock
import software.amazon.smithy.rust.codegen.core.rustlang.rustBlockTemplate
import software.amazon.smithy.rust.codegen.core.rustlang.rustTemplate
import software.amazon.smithy.rust.codegen.core.rustlang.withBlock
import software.amazon.smithy.rust.codegen.core.smithy.CodegenContext
import software.amazon.smithy.rust.codegen.core.smithy.RuntimeType
import software.amazon.smithy.rust.codegen.core.smithy.generators.UnionGenerator
import software.amazon.smithy.rust.codegen.core.smithy.generators.renderUnknownVariant
import software.amazon.smithy.rust.codegen.core.smithy.isOptional
import software.amazon.smithy.rust.codegen.core.smithy.protocols.HttpBindingResolver
import software.amazon.smithy.rust.codegen.core.smithy.protocols.HttpLocation
import software.amazon.smithy.rust.codegen.core.smithy.protocols.XmlMemberIndex
import software.amazon.smithy.rust.codegen.core.smithy.protocols.XmlNameIndex
import software.amazon.smithy.rust.codegen.core.smithy.protocols.serialize.XmlBindingTraitSerializerGenerator.Ctx
import software.amazon.smithy.rust.codegen.core.util.dq
import software.amazon.smithy.rust.codegen.core.util.getTrait
import software.amazon.smithy.rust.codegen.core.util.hasTrait
import software.amazon.smithy.rust.codegen.core.util.isTargetUnit
import software.amazon.smithy.rust.codegen.core.util.letIf
import software.amazon.smithy.rust.codegen.core.util.outputShape
import software.amazon.smithy.utils.StringUtils

class Ec2QuerySerializerGenerator(codegenContext: CodegenContext, private val httpBindingResolver: HttpBindingResolver,
) : QuerySerializerGenerator(codegenContext) {


    override val protocolName: String get() = "EC2 Query"
    private val xmlIndex = XmlNameIndex.of(model)
    private val rootNamespace = codegenContext.serviceShape.getTrait<XmlNamespaceTrait>()
    private val codegenTarget = codegenContext.target
    private val util = SerializerUtil(model, symbolProvider)

    override fun MemberShape.queryKeyName(prioritizedFallback: String?): String =
        getTrait<Ec2QueryNameTrait>()?.value
            ?: getTrait<XmlNameTrait>()?.value?.let { StringUtils.capitalize(it) }
            ?: StringUtils.capitalize(memberName)

    override fun MemberShape.isFlattened(): Boolean = true

    private fun OperationShape.responseBodyMembers(): XmlMemberIndex =
        XmlMemberIndex.fromMembers(httpBindingResolver.responseMembers(this, HttpLocation.DOCUMENT))

    override fun operationOutputSerializer(operationShape: OperationShape): RuntimeType? {
        val outputShape = operationShape.outputShape(model)
        val xmlMembers = operationShape.responseBodyMembers()
        if (xmlMembers.isEmpty()) {
            return null
        }
        val operationXmlName =
            xmlIndex.operationOutputShapeName(operationShape)
                ?: throw CodegenException("operation must have a name if it has members")
        return protocolFunctions.serializeFn(operationShape, fnNameSuffix = "output") { fnName ->
            rustBlockTemplate(
                "pub fn $fnName(output: &#{target}) -> Result<String, #{Error}>",
                *codegenScope, "target" to symbolProvider.toSymbol(outputShape),
            ) {
                rust("let mut out = String::new();")
                // Create a scope for writer. This ensures that:
                // - The writer is dropped before returning the string
                // - All closing tags get written
                rustBlock("") {
                    rustTemplate(
                        """
                        let mut writer = #{XmlWriter}::new(&mut out);
                        ##[allow(unused_mut)]
                        let mut root = writer.start_el(${operationXmlName.dq()})${
                            outputShape.xmlNamespace(root = true).apply()
                        };
                        """,
                        *codegenScope,
                    )
                    serializeStructure(outputShape, xmlMembers, Ctx.Element("root", "output"))
                }
                rustTemplate("Ok(out)", *codegenScope)
            }
        }
    }

    private fun XmlNamespaceTrait?.apply(): String {
        this ?: return ""
        val prefix = prefix.map { prefix -> "Some(${prefix.dq()})" }.orElse("None")
        return ".write_ns(${uri.dq()}, $prefix)"
    }

    private fun RustWriter.serializeStructure(
        structureShape: StructureShape,
        members: XmlMemberIndex,
        ctx: Ctx.Element,
        fnNameSuffix: String? = null,
    ) {
        val structureSymbol = symbolProvider.toSymbol(structureShape)
        val structureSerializer =
            protocolFunctions.serializeFn(structureShape, fnNameSuffix = fnNameSuffix) { fnName ->
                rustBlockTemplate(
                    "pub fn $fnName(input: &#{Input}, writer: #{ElWriter}) -> Result<(), #{Error}>",
                    "Input" to structureSymbol,
                    *codegenScope,
                ) {
                    if (!members.isNotEmpty()) {
                        // removed unused warning if there are no fields we're going to read
                        rust("let _ = input;")
                    }
                    structureInner(members, Ctx.Element("writer", "&input"))
                    rust("Ok(())")
                }
            }
        rust("#T(${ctx.input}, ${ctx.elementWriter})?", structureSerializer)
    }

    private fun RustWriter.structureInner(
        members: XmlMemberIndex,
        ctx: Ctx.Element,
    ) {
        if (members.attributeMembers.isNotEmpty()) {
            rust("let mut ${ctx.elementWriter} = ${ctx.elementWriter};")
        }
        members.attributeMembers.forEach { member ->
            handleOptional(member, ctx.scopedTo(member)) { ctx ->
                withBlock("${ctx.elementWriter}.write_attribute(${xmlIndex.memberName(member).dq()},", ");") {
                    serializeRawMember(member, ctx.input)
                }
            }
        }
        Attribute.AllowUnusedMut.render(this)
        rust("let mut scope = ${ctx.elementWriter}.finish();")
        val scopeCtx = Ctx.Scope("scope", ctx.input)
        members.dataMembers.forEach { member ->
            serializeMember(member, scopeCtx.scopedTo(member), null)
        }
        rust("scope.finish();")
    }

    private fun RustWriter.serializeFlatList(
        member: MemberShape,
        listShape: CollectionShape,
        ctx: Ctx.Scope,
    ) {
        val itemName = safeName("list_item")
        rustBlock("for $itemName in ${ctx.input}") {
            serializeMember(listShape.member, ctx.copy(input = itemName), xmlIndex.memberName(member))
        }
    }

    private fun RustWriter.serializeList(
        listShape: CollectionShape,
        ctx: Ctx.Scope,
    ) {
        val itemName = safeName("list_item")
        rustBlock("for $itemName in ${ctx.input}") {
            serializeMember(listShape.member, ctx.copy(input = itemName))
        }
    }

    private fun RustWriter.serializeMap(
        mapShape: MapShape,
        entryName: String,
        ctx: Ctx.Scope,
    ) {
        val key = safeName("key")
        val value = safeName("value")
        rustBlock("for ($key, $value) in ${ctx.input}") {
            rust("""let mut entry = ${ctx.scopeWriter}.start_el(${entryName.dq()}).finish();""")
            serializeMember(mapShape.key, ctx.copy(scopeWriter = "entry", input = key))
            serializeMember(mapShape.value, ctx.copy(scopeWriter = "entry", input = value))
        }
    }

    @Suppress("NAME_SHADOWING")
    private fun RustWriter.serializeMember(
        memberShape: MemberShape,
        ctx: Ctx.Scope,
        rootNameOverride: String? = null,
    ) {
        val target = model.expectShape(memberShape.target)
        val xmlName = rootNameOverride ?: xmlIndex.memberName(memberShape)
        val ns = memberShape.xmlNamespace(root = false).apply()
        handleOptional(memberShape, ctx) { ctx ->
            when (target) {
                is StringShape, is BooleanShape, is NumberShape, is TimestampShape, is BlobShape -> {
                    rust("let mut inner_writer = ${ctx.scopeWriter}.start_el(${xmlName.dq()})$ns.finish();")
                    withBlock("inner_writer.data(", ");") {
                        serializeRawMember(memberShape, ctx.input)
                    }
                }

                is CollectionShape ->
                    if (memberShape.hasTrait<XmlFlattenedTrait>()) {
                        serializeFlatList(memberShape, target, ctx)
                    } else {
                        rust("let mut inner_writer = ${ctx.scopeWriter}.start_el(${xmlName.dq()})$ns.finish();")
                        serializeList(target, Ctx.Scope("inner_writer", ctx.input))
                    }

                is MapShape ->
                    if (memberShape.hasTrait<XmlFlattenedTrait>()) {
                        serializeMap(target, xmlIndex.memberName(memberShape), ctx)
                    } else {
                        rust("let mut inner_writer = ${ctx.scopeWriter}.start_el(${xmlName.dq()})$ns.finish();")
                        serializeMap(target, "entry", Ctx.Scope("inner_writer", ctx.input))
                    }

                is StructureShape -> {
                    // We call serializeStructure only when target.members() is nonempty.
                    // If it were empty, serializeStructure would generate the following code:
                    //   pub fn serialize_structure_crate_model_unit(
                    //       input: &crate::model::Unit,
                    //       writer: aws_smithy_xml::encode::ElWriter,
                    //   ) -> Result<(), aws_smithy_http::operation::error::SerializationError> {
                    //       let _ = input;
                    //       #[allow(unused_mut)]
                    //       let mut scope = writer.finish();
                    //       scope.finish();
                    //       Ok(())
                    //   }
                    // However, this would cause a compilation error at a call site because it cannot
                    // extract data out of the Unit type that corresponds to the variable "input" above.
                    if (target.members().isEmpty()) {
                        rust("${ctx.scopeWriter}.start_el(${xmlName.dq()})$ns.finish();")
                    } else {
                        rust("let inner_writer = ${ctx.scopeWriter}.start_el(${xmlName.dq()})$ns;")
                        serializeStructure(
                            target,
                            XmlMemberIndex.fromMembers(target.members().toList()),
                            Ctx.Element("inner_writer", ctx.input),
                        )
                    }
                }

                is UnionShape -> {
                    rust("let inner_writer = ${ctx.scopeWriter}.start_el(${xmlName.dq()})$ns;")
                    serializeUnion(target, Ctx.Element("inner_writer", ctx.input))
                }

                else -> TODO(target.toString())
            }
        }
    }

    private fun RustWriter.serializeUnion(
        unionShape: UnionShape,
        ctx: Ctx.Element,
    ) {
        val unionSymbol = symbolProvider.toSymbol(unionShape)
        val structureSerializer =
            protocolFunctions.serializeFn(unionShape) { fnName ->
                rustBlockTemplate(
                    "pub fn $fnName(input: &#{Input}, writer: #{ElWriter}) -> Result<(), #{Error}>",
                    "Input" to unionSymbol,
                    *codegenScope,
                ) {
                    rust("let mut scope_writer = writer.finish();")
                    rustBlock("match input") {
                        val members = unionShape.members()
                        members.forEach { member ->
                            val variantName =
                                if (member.isTargetUnit()) {
                                    "${symbolProvider.toMemberName(member)}"
                                } else {
                                    "${symbolProvider.toMemberName(member)}(inner)"
                                }
                            withBlock("#T::$variantName =>", ",", unionSymbol) {
                                serializeMember(member, Ctx.Scope("scope_writer", "inner"))
                            }
                        }

                        if (codegenTarget.renderUnknownVariant()) {
                            rustTemplate(
                                "#{Union}::${UnionGenerator.UNKNOWN_VARIANT_NAME} => return Err(#{Error}::unknown_variant(${unionSymbol.name.dq()}))",
                                "Union" to unionSymbol,
                                *codegenScope,
                            )
                        }
                    }
                    rust("Ok(())")
                }
            }
        rust("#T(${ctx.input}, ${ctx.elementWriter})?", structureSerializer)
    }

    private fun Ctx.Scope.scopedTo(member: MemberShape) =
        this.copy(input = "$input.${symbolProvider.toMemberName(member)}")

    private fun RustWriter.serializeRawMember(
        member: MemberShape,
        input: String,
    ) {
        when (model.expectShape(member.target)) {
            is StringShape -> {
                // The `input` expression always evaluates to a reference type at this point, but if it does so because
                // it's preceded by the `&` operator, calling `as_str()` on it will upset Clippy.
                val dereferenced =
                    if (input.startsWith("&")) {
                        autoDeref(input)
                    } else {
                        input
                    }
                rust("$dereferenced.as_str()")
            }

            is BooleanShape, is NumberShape -> {
                rust(
                    "#T::from(${autoDeref(input)}).encode()",
                    RuntimeType.smithyTypes(runtimeConfig).resolve("primitive::Encoder"),
                )
            }

            is BlobShape -> rust("#T($input.as_ref()).as_ref()", RuntimeType.base64Encode(runtimeConfig))
            is TimestampShape -> {
                val timestampFormat =
                    httpBindingResolver.timestampFormat(
                        member,
                        HttpLocation.DOCUMENT,
                        TimestampFormatTrait.Format.DATE_TIME, model,
                    )
                val timestampFormatType = RuntimeType.parseTimestampFormat(codegenTarget, runtimeConfig, timestampFormat)
                rust("$input.fmt(#T)?.as_ref()", timestampFormatType)
            }

            else -> TODO(member.toString())
        }
    }

    private fun Ctx.Element.scopedTo(member: MemberShape) =
        this.copy(input = "$input.${symbolProvider.toMemberName(member)}")

    private fun <T : Ctx> RustWriter.handleOptional(
        member: MemberShape,
        ctx: T,
        inner: RustWriter.(T) -> Unit,
    ) {
        val memberSymbol = symbolProvider.toSymbol(member)
        if (memberSymbol.isOptional()) {
            val tmp = safeName()
            val target = model.expectShape(member.target)
            val pattern =
                if (target.isStructureShape && target.members().isEmpty()) {
                    // In this case, we mark a variable captured in the if-let
                    // expression as unused to prevent the warning coming
                    // from the following code generated by handleOptional:
                    //   if let Some(var_2) = &input.input {
                    //       scope.start_el("input").finish();
                    //   }
                    // where var_2 above is unused.
                    "Some(_$tmp)"
                } else {
                    "Some($tmp)"
                }
            rustBlock("if let $pattern = ${ctx.input}") {
                inner(Ctx.updateInput(ctx, tmp))
            }
        } else {
            with(util) {
                val valueExpression =
                    if (ctx.input.startsWith("&")) {
                        software.amazon.smithy.rust.codegen.core.smithy.protocols.serialize.ValueExpression.Reference(ctx.input)
                    } else {
                        software.amazon.smithy.rust.codegen.core.smithy.protocols.serialize.ValueExpression.Value(ctx.input)
                    }
                ignoreDefaultsForNumbersAndBools(member, valueExpression) {
                    inner(ctx)
                }
            }
        }
    }

    private fun Shape.xmlNamespace(root: Boolean): XmlNamespaceTrait? {
        return this.getTrait<XmlNamespaceTrait>().letIf(root) { it ?: rootNamespace }
    }

    override fun serverErrorSerializer(shape: ShapeId): RuntimeType {
        TODO("Not yet implemented")
    }

    override fun RustWriter.serializeCollection(
        memberContext: MemberContext,
        context: Context<CollectionShape>,
    ) {
        rustBlock("if !${context.valueExpression.asRef()}.is_empty()") {
            super.serializeCollectionInner(memberContext, context, this)
        }
    }
}
