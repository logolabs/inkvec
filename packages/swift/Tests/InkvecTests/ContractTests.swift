// The cross-language contract, bindings/contract/cases.json, checked the way
// crates/inkvec/tests/contract.rs checks it:
//
//   - `expect`: the reported width and height, or the kind of error;
//   - `svg[target]`: the SVG's length and SHA-256 on this build target (`Inkvec.buildTarget`).
//     A target without recorded hashes is reported, not failed, unless
//     INKVEC_CONTRACT_REQUIRE_HASH=1;
//   - `same_svg_as`: identical SVG to the named case.
//
// Every case runs through the raw JSON entry points. Every case that traces runs a second
// time through the typed `InkvecOptions`, decoded from the case's options: the same SVG
// proves the generated struct carries each option under the schema's name. No option is
// named in this file, so an option added or renamed in Rust changes nothing here.

import Foundation
import Inkvec
import XCTest

final class ContractTests: XCTestCase {
    private func cases() throws -> [[String: Any]] {
        let doc = try XCTUnwrap(Repo.json("bindings/contract/cases.json") as? [String: Any])
        return try XCTUnwrap(doc["cases"] as? [[String: Any]])
    }

    /// One case through the raw entry points: the trace, or the error it raised.
    private func run(_ c: [String: Any], optionsJSON: String) throws -> Result<Traced, InkvecError> {
        let input = try Data(contentsOf: Repo.contract.appendingPathComponent(c["input"] as! String))
        do {
            if c["form"] as? String == "rgba" {
                return .success(try Inkvec.traceRGBA(
                    input, width: c["width"] as! Int, height: c["height"] as! Int, optionsJSON: optionsJSON))
            }
            return .success(try Inkvec.trace(input, optionsJSON: optionsJSON))
        } catch let error as InkvecError {
            return .failure(error)
        }
    }

    /// One case through the typed entry points.
    private func run(_ c: [String: Any], options: InkvecOptions) throws -> Traced {
        let input = try Data(contentsOf: Repo.contract.appendingPathComponent(c["input"] as! String))
        if c["form"] as? String == "rgba" {
            return try Inkvec.traceRGBA(input, width: c["width"] as! Int, height: c["height"] as! Int, options: options)
        }
        return try Inkvec.trace(input, options: options)
    }

    func testTheContract() throws {
        let target = Inkvec.buildTarget
        var svgs: [String: String] = [:]
        var unhashed: [String] = []
        let all = try cases()
        XCTAssertFalse(all.isEmpty)

        for c in all {
            let name = c["name"] as! String
            let options = try XCTUnwrap(c["options"] as? [String: Any], name)
            let expect = try XCTUnwrap(c["expect"] as? [String: Any], name)
            let json = String(
                decoding: try JSONSerialization.data(withJSONObject: options, options: [.sortedKeys]), as: UTF8.self)

            switch try run(c, optionsJSON: json) {
            case .failure(let error):
                XCTAssertEqual(error.kind, expect["error"] as? String, "\(name): \(error.message)")
                XCTAssertFalse(error.message.isEmpty, name)
            case .success(let traced):
                XCTAssertNil(expect["error"], "\(name): traced without the expected error")
                XCTAssertEqual(traced.width, expect["width"] as? Int, name)
                XCTAssertEqual(traced.height, expect["height"] as? Int, name)
                svgs[name] = traced.svg

                let bytes = traced.svg.utf8.count
                let sha = SHA256.hex(traced.svg)
                if let want = (c["svg"] as? [String: Any])?[target] as? [String: Any] {
                    XCTAssertEqual(bytes, want["bytes"] as? Int, "\(name): SVG length on \(target)")
                    XCTAssertEqual(sha, want["sha256"] as? String, "\(name): SVG hash on \(target)")
                } else {
                    unhashed.append("\(name): {\"bytes\": \(bytes), \"sha256\": \"\(sha)\"}")
                }

                // The typed path: every option of the case survives decoding into InkvecOptions
                // and encoding back, and traces to the same bytes.
                let typed = try JSONDecoder().decode(InkvecOptions.self, from: Data(json.utf8))
                let back = try XCTUnwrap(
                    JSONSerialization.jsonObject(with: Data(typed.jsonString().utf8)) as? [String: Any])
                XCTAssertEqual(
                    Set(back.keys), Set(options.keys),
                    "\(name): InkvecOptions dropped an option; run python bindings/codegen/generate.py")
                XCTAssertEqual(SHA256.hex(try run(c, options: typed).svg), sha, "\(name): typed options")
            }
        }

        for c in all {
            guard let other = c["same_svg_as"] as? String else { continue }
            let name = c["name"] as! String
            XCTAssertNotNil(svgs[other], "\(other) produced no SVG")
            XCTAssertEqual(svgs[name], svgs[other], "\(name) vs \(other)")
        }

        if !unhashed.isEmpty {
            let message = "no SVG hashes recorded for \(target); checked everything else:\n  "
                + unhashed.joined(separator: "\n  ")
            if ProcessInfo.processInfo.environment["INKVEC_CONTRACT_REQUIRE_HASH"] == "1" {
                XCTFail(message)
            } else {
                print("note: \(message)")
            }
        }
    }

    func testTheHashFunction() {
        // FIPS 180-4 test vectors, so a contract failure is never the digest's fault.
        XCTAssertEqual(SHA256.hex(""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        XCTAssertEqual(SHA256.hex("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        XCTAssertEqual(
            SHA256.hex("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1")
    }
}
