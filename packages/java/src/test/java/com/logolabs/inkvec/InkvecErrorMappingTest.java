package com.logolabs.inkvec;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.Test;

/**
 * Every C ABI error kind, reached through the Java API. Mirrors the mapping table in
 * docs/BINDINGS.md ("Errors"): invalid_image / invalid_options / internal become dedicated
 * {@link InkvecException} subclasses; a binding-level misuse (here: a JNA argument problem)
 * would surface as a plain {@link RuntimeException}, never one of these three.
 */
final class InkvecErrorMappingTest {

    @Test
    void notAnImageIsInvalidImage() {
        InvalidImageException ex = assertThrows(InvalidImageException.class, () -> Inkvec.trace(new byte[] {1, 2, 3}));
        assertFalse(ex.getMessage().isEmpty());
    }

    @Test
    void rgbaSizeMismatchIsInvalidImage() {
        byte[] pixels = new byte[16 * 16 * 4];
        assertThrows(InvalidImageException.class, () -> Inkvec.traceRgba(pixels, 15, 16));
    }

    @Test
    void unknownOptionKeyIsInvalidOptions_typoInDefaultsPassthrough() {
        byte[] png = ContractSupport.input(ContractSupport.caseByName("tiny_defaults"));
        // The exact case from the shared contract: "colours" (not "colors") is unknown, and
        // Options::from_json rejects it by name rather than silently ignoring it -- proof
        // that an option added to Rust today is usable from raw JSON before InkvecOptions is
        // regenerated for it, and that a typo does not silently fall back to the default.
        InvalidOptionsException ex =
                assertThrows(InvalidOptionsException.class, () -> Inkvec.trace(png, "{\"colours\": 8}"));
        assertTrue(ex.getMessage().toLowerCase().contains("colours"), ex.getMessage());
    }

    @Test
    void malformedJsonIsInvalidOptions() {
        byte[] png = ContractSupport.input(ContractSupport.caseByName("tiny_defaults"));
        assertThrows(InvalidOptionsException.class, () -> Inkvec.trace(png, "{not json"));
    }

    @Test
    void outOfRangeTypedOptionIsInvalidOptions() {
        byte[] png = ContractSupport.input(ContractSupport.caseByName("tiny_defaults"));
        InkvecOptions bad = InkvecOptions.builder().colors(0).build();
        assertThrows(InvalidOptionsException.class, () -> Inkvec.trace(png, bad));
    }

    @Test
    void wrongTypeIsInvalidOptions() {
        byte[] png = ContractSupport.input(ContractSupport.caseByName("tiny_defaults"));
        assertThrows(InvalidOptionsException.class, () -> Inkvec.trace(png, "{\"cutout\": \"yes\"}"));
    }

    @Test
    void everyExceptionExtendsInkvecException() {
        assertTrue(InkvecException.class.isAssignableFrom(InvalidImageException.class));
        assertTrue(InkvecException.class.isAssignableFrom(InvalidOptionsException.class));
        assertTrue(InkvecException.class.isAssignableFrom(InternalException.class));
    }

    @Test
    void nullImageThrowsWithoutCrossingIntoNative() {
        assertThrows(NullPointerException.class, () -> Inkvec.trace((byte[]) null));
    }
}
