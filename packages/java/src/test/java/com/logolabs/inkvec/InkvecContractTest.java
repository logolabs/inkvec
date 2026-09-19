package com.logolabs.inkvec;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

import com.fasterxml.jackson.databind.JsonNode;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.stream.Stream;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.Arguments;
import org.junit.jupiter.params.provider.MethodSource;

/**
 * Runs every case in {@code bindings/contract/cases.json} through {@link Inkvec}, exactly
 * like every other binding's contract test (see docs/BINDINGS.md, "The contract fixtures").
 */
final class InkvecContractTest {

    static Stream<Arguments> cases() {
        return ContractSupport.cases().stream().map(c -> Arguments.of(c.get("name").asText(), c));
    }

    @ParameterizedTest(name = "{0}")
    @MethodSource("cases")
    void contract(String name, JsonNode c) {
        JsonNode expect = c.get("expect");
        if (expect.has("error")) {
            InkvecException ex = assertThrows(InkvecException.class, () -> ContractSupport.run(c), name);
            assertEquals(errorClass(expect.get("error").asText()), ex.getClass(), name);
            return;
        }
        TraceResult r = ContractSupport.run(c);
        assertEquals(expect.get("width").asInt(), r.width(), name);
        assertEquals(expect.get("height").asInt(), r.height(), name);

        byte[] svg = r.svg().getBytes(StandardCharsets.UTF_8);
        JsonNode want = c.has("svg") ? c.get("svg").get(Inkvec.buildTarget()) : null;
        if (want != null) {
            assertEquals(want.get("bytes").asInt(), svg.length, name + ": byte length");
            assertEquals(want.get("sha256").asText(), ContractSupport.sha256(svg), name + ": sha256");
        }
    }

    @ParameterizedTest(name = "{0} == {1}")
    @MethodSource("pairedCases")
    void samePairsAgree(String name, String other) {
        TraceResult a = ContractSupport.run(ContractSupport.caseByName(name));
        TraceResult b = ContractSupport.run(ContractSupport.caseByName(other));
        assertEquals(a.svg(), b.svg(), name + " vs " + other);
    }

    static Stream<Arguments> pairedCases() {
        List<JsonNode> cases = ContractSupport.cases();
        return cases.stream()
                .filter(c -> c.has("same_svg_as"))
                .map(c -> Arguments.of(c.get("name").asText(), c.get("same_svg_as").asText()));
    }

    private static Class<? extends InkvecException> errorClass(String kind) {
        switch (kind) {
            case "invalid_image":
                return InvalidImageException.class;
            case "invalid_options":
                return InvalidOptionsException.class;
            case "internal":
                return InternalException.class;
            default:
                throw new IllegalArgumentException("unknown error kind: " + kind);
        }
    }
}
