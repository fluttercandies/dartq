// messages-example: analyzer/messages.yaml CompileTimeErrorCode.mixinModifierMixinApplicationClassWithMultipleMixins
mixin M1 {}

mixin M2 {}

mixin class C = Object with M1, M2;
