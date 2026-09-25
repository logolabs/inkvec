package com.logolabs.inkvec;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import org.junit.jupiter.api.Test;

/** Every documented way to hand Inkvec an image, and the typed/JSON/default option paths. */
final class InkvecInputKindsTest {

    @Test
    void encodedBytesWithDefaults() {
        TraceResult r = Inkvec.trace(ContractSupport.input(ContractSupport.caseByName("tiny_defaults")));
        assertEquals(96, r.width());
        assertEquals(96, r.height());
        assertTrue(r.svg().contains("<svg"));
    }

    @Test
    void rawRgbaMatchesEncoded() {
        byte[] rgba = ContractSupport.input(ContractSupport.caseByName("tiny_rgba_defaults"));
        byte[] png = ContractSupport.input(ContractSupport.caseByName("tiny_defaults"));
        TraceResult fromRgba = Inkvec.traceRgba(rgba, 96, 96);
        TraceResult fromPng = Inkvec.trace(png);
        assertEquals(fromPng.svg(), fromRgba.svg());
    }

    @Test
    void pathOverloadReadsTheFile() throws IOException {
        Path p = ContractSupport.CONTRACT.resolve("tiny.png");
        TraceResult fromPath = Inkvec.trace(p);
        TraceResult fromBytes = Inkvec.trace(Files.readAllBytes(p));
        assertEquals(fromBytes.svg(), fromPath.svg());
    }

    @Test
    void typedOptionsAndEquivalentJsonAgree() {
        byte[] png = ContractSupport.input(ContractSupport.caseByName("tiny_defaults"));
        InkvecOptions typed = InkvecOptions.builder().colors(8).minify(true).noBackground(true).build();
        TraceResult fromTyped = Inkvec.trace(png, typed);
        TraceResult fromJson = Inkvec.trace(png, "{\"colors\": 8, \"minify\": true, \"no_background\": true}");
        assertEquals(fromJson.svg(), fromTyped.svg());
    }

    @Test
    void stringOptionsAreWrittenAsEscapedJson() {
        String json = InkvecOptions.builder().mergeColors("#f00>#00f,#0a0=@1;\"q\"\\\n\u0001").build().toJson();
        assertTrue(
            json.contains("\"merge_colors\":\"#f00>#00f,#0a0=@1;\\\"q\\\"\\\\\\n\\u0001\""), json);
        assertTrue(InkvecOptions.defaults().toJson().contains("\"merge_colors\":\"\""));
    }

    @Test
    void defaultsBuilderReproducesNoOptionsTrace() {
        byte[] png = ContractSupport.input(ContractSupport.caseByName("tiny_defaults"));
        TraceResult fromDefaults = Inkvec.trace(png, InkvecOptions.defaults());
        TraceResult fromNull = Inkvec.trace(png);
        assertEquals(fromNull.svg(), fromDefaults.svg());
    }

    @Test
    void nullOptionsJsonMeansDefaults() {
        byte[] png = ContractSupport.input(ContractSupport.caseByName("tiny_defaults"));
        TraceResult a = Inkvec.trace(png, (String) null);
        TraceResult b = Inkvec.trace(png);
        assertEquals(a.svg(), b.svg());
    }

    @Test
    void identification() {
        assertNotNull(Inkvec.version());
        assertTrue(Inkvec.version().matches("\\d+\\.\\d+\\.\\d+.*"), Inkvec.version());
        assertNotNull(Inkvec.buildTarget());
        assertTrue(Inkvec.abiVersion() >= 1);
    }

    @Test
    void optionsSchemaAndDefaultsAreValidJsonNamingEveryOption() {
        String schema = Inkvec.optionsSchema();
        String defaults = Inkvec.defaults();
        for (String name : new String[] {
            "precision", "min_area", "colors", "merge", "max_dim", "time_budget", "margin",
            "no_background", "minify", "native_alpha", "cutout", "content_units", "harmonize",
            "harmonize_threshold", "merge_colors"
        }) {
            assertTrue(schema.contains("\"" + name + "\""), name + " missing from schema");
            assertTrue(defaults.contains("\"" + name + "\""), name + " missing from defaults");
        }
    }
}
