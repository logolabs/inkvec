package com.logolabs.inkvec;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.List;
import java.util.stream.StreamSupport;

/**
 * Loads {@code bindings/contract/cases.json}, the fixtures every Inkvec binding's tests run
 * against (see docs/BINDINGS.md, "The contract fixtures"). Test-only: the main binding
 * carries no JSON library, so this is where one is allowed.
 */
final class ContractSupport {
    static final Path ROOT = Paths.get(System.getProperty("inkvec.repoRoot", "../..")).toAbsolutePath().normalize();
    static final Path CONTRACT = ROOT.resolve("bindings").resolve("contract");

    private static final ObjectMapper MAPPER = new ObjectMapper();

    private ContractSupport() {
    }

    static List<JsonNode> cases() {
        try {
            JsonNode root = MAPPER.readTree(CONTRACT.resolve("cases.json").toFile());
            List<JsonNode> out = new ArrayList<>();
            StreamSupport.stream(root.get("cases").spliterator(), false).forEach(out::add);
            return out;
        } catch (IOException e) {
            throw new RuntimeException("reading cases.json", e);
        }
    }

    static JsonNode caseByName(String name) {
        for (JsonNode c : cases()) {
            if (c.get("name").asText().equals(name)) {
                return c;
            }
        }
        throw new IllegalArgumentException("no such case: " + name);
    }

    static byte[] input(JsonNode c) {
        try {
            return Files.readAllBytes(CONTRACT.resolve(c.get("input").asText()));
        } catch (IOException e) {
            throw new RuntimeException(e);
        }
    }

    /** Runs one contract case through {@link Inkvec}, returning the SVG, or null on an error. */
    static TraceResult run(JsonNode c) {
        byte[] data = input(c);
        String optionsJson = c.get("options").toString();
        if ("encoded".equals(c.get("form").asText())) {
            return Inkvec.trace(data, optionsJson);
        }
        return Inkvec.traceRgba(data, c.get("width").asInt(), c.get("height").asInt(), optionsJson);
    }

    static String sha256(byte[] data) {
        try {
            MessageDigest digest = MessageDigest.getInstance("SHA-256");
            byte[] hash = digest.digest(data);
            StringBuilder sb = new StringBuilder(hash.length * 2);
            for (byte b : hash) {
                sb.append(String.format("%02x", b));
            }
            return sb.toString();
        } catch (NoSuchAlgorithmException e) {
            throw new RuntimeException(e);
        }
    }
}
