package com.logolabs.inkvec;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import javax.xml.parsers.DocumentBuilder;
import javax.xml.parsers.DocumentBuilderFactory;
import org.junit.jupiter.api.Test;
import org.w3c.dom.Element;
import org.w3c.dom.Node;
import org.w3c.dom.NodeList;

/**
 * White artwork on a transparent ground: with native_alpha on (the default), it must trace
 * in its own colour with no background rectangle covering the canvas -- the same check
 * crates/inkvec-py/tests/test_inkvec.py makes for the Python package, against the same
 * fixture ({@code bindings/contract/white_on_clear.rgba}, 64x64).
 */
final class InkvecWhiteOnTransparentTest {

    @Test
    void defaultsPaintNoBackground() {
        assertNoBackground(InkvecOptions.defaults());
    }

    @Test
    void compositedWithCutoutAlsoPaintsNoBackground() {
        assertNoBackground(InkvecOptions.builder().nativeAlpha(false).cutout(true).build());
    }

    private void assertNoBackground(InkvecOptions options) {
        byte[] rgba = ContractSupport.input(ContractSupport.caseByName("clear_rgba_defaults"));
        TraceResult r = Inkvec.traceRgba(rgba, 64, 64, options);
        List<Element> shapes = shapeElements(r.svg());
        assertFalse(shapes.isEmpty(), r.svg());
        for (Element el : shapes) {
            if (el.getTagName().equals("rect")) {
                double w = Double.parseDouble(el.getAttribute("width"));
                double h = Double.parseDouble(el.getAttribute("height"));
                assertTrue(w < 60 && h < 60, "a rect spans the canvas: " + r.svg());
            }
            String d = el.getAttribute("d");
            assertFalse(d.contains("-0.50,-0.50"), "a path spans the canvas: " + r.svg());
            String paint = "none".equals(el.getAttribute("fill")) ? el.getAttribute("stroke") : el.getAttribute("fill");
            assertTrue(paint.equals("#ffffff") || paint.equals("#fff"), "unexpected paint " + paint + ": " + r.svg());
        }
    }

    @Test
    void opaqueInputStillPaintsItsCanvas() {
        byte[] png = ContractSupport.input(ContractSupport.caseByName("tiny_defaults"));
        TraceResult r = Inkvec.trace(png);
        boolean fullCanvasRect =
                shapeElements(r.svg()).stream()
                        .anyMatch(
                                el -> el.getTagName().equals("rect")
                                        && Double.parseDouble(el.getAttribute("width")) >= 96);
        assertTrue(fullCanvasRect, r.svg());
    }

    private static List<Element> shapeElements(String svg) {
        try {
            DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
            factory.setNamespaceAware(false);
            DocumentBuilder builder = factory.newDocumentBuilder();
            org.w3c.dom.Document doc =
                    builder.parse(new ByteArrayInputStream(svg.getBytes(StandardCharsets.UTF_8)));
            List<Element> out = new ArrayList<>();
            NodeList all = doc.getElementsByTagName("*");
            for (int i = 0; i < all.getLength(); i++) {
                Node n = all.item(i);
                if (n instanceof Element) {
                    String tag = ((Element) n).getTagName();
                    if (tag.equals("path") || tag.equals("rect") || tag.equals("circle") || tag.equals("ellipse")) {
                        out.add((Element) n);
                    }
                }
            }
            return out;
        } catch (Exception e) {
            throw new RuntimeException("parsing SVG:\n" + svg, e);
        }
    }
}
