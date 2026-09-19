// The Swift API around the contract: identification, options and their schema, error
// mapping, file input, threads, and the header copy. Like ContractTests, no option is named
// here: everything about options is read from the schema and the library.

import Foundation
import Inkvec
import XCTest

final class APITests: XCTestCase {
    private func tiny() throws -> Data { try Repo.data("bindings/contract/tiny.png") }

    private func object(_ json: String) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any])
    }

    // MARK: Identification

    func testTheVersionIsTheWorkspaceVersion() throws {
        let cargo = String(decoding: try Repo.data("Cargo.toml"), as: UTF8.self)
        let line = cargo.split(whereSeparator: \.isNewline).first { $0.hasPrefix("version = ") }
        XCTAssertEqual(line.map(String.init), "version = \"\(Inkvec.version)\"")
    }

    func testTheBuildTargetNamesArchitectureAndSystem() {
        let parts = Inkvec.buildTarget.split(separator: "-")
        XCTAssertGreaterThanOrEqual(parts.count, 2, Inkvec.buildTarget)
        #if arch(x86_64)
        XCTAssertEqual(parts.first, "x86_64")
        #elseif arch(arm64)
        XCTAssertEqual(parts.first, "aarch64")
        #endif
    }

    // MARK: Options

    /// A JSON value in one canonical spelling (sorted keys), for comparing parsed documents
    /// without Objective-C bridging, which Linux does not have.
    private func canonical(_ value: Any) throws -> String {
        String(decoding: try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]), as: UTF8.self)
    }

    private func decode(_ json: Data) throws -> InkvecOptions {
        try JSONDecoder().decode(InkvecOptions.self, from: json)
    }

    func testTheLibrarySchemaIsTheCommittedSchema() throws {
        XCTAssertEqual(
            try canonical(object(Inkvec.optionsSchema)),
            try canonical(Repo.json("bindings/options.schema.json")))
    }

    func testTheGeneratedOptionsCoverTheSchema() throws {
        let properties = try XCTUnwrap(try object(Inkvec.optionsSchema)["properties"] as? [String: Any])
        XCTAssertEqual(Set(try object(Inkvec.defaultsJSON).keys), Set(properties.keys))

        // Inkvec.defaults is the library's defaults decoded into InkvecOptions. Encoded again it
        // must name every option in the schema, under the schema's name ...
        XCTAssertEqual(
            Set(try object(Inkvec.defaults.jsonString()).keys), Set(properties.keys),
            "InkvecOptions.generated.swift is stale; run python bindings/codegen/generate.py")
        // ... with the same values, which are also the schema's defaults.
        XCTAssertEqual(try decode(Data(Inkvec.defaults.jsonString().utf8)), Inkvec.defaults)
        let schemaDefaults = try properties.mapValues { property -> Any in
            try XCTUnwrap((property as? [String: Any])?["default"])
        }
        XCTAssertEqual(try decode(JSONSerialization.data(withJSONObject: schemaDefaults)), Inkvec.defaults)
    }

    func testUnsetOptionsAreLeftOut() throws {
        XCTAssertEqual(try InkvecOptions().jsonString(), "{}")
        XCTAssertEqual(InkvecOptions(), try decode(Data("{}".utf8)))
    }

    func testMissingOptionsMeanTheDefaults() throws {
        let png = try tiny()
        let reference = try Inkvec.trace(png)
        XCTAssertEqual(try Inkvec.trace(png, optionsJSON: nil), reference)
        XCTAssertEqual(try Inkvec.trace(png, optionsJSON: ""), reference)
        XCTAssertEqual(try Inkvec.trace(png, optionsJSON: "{}"), reference)
        XCTAssertEqual(try Inkvec.trace(png, optionsJSON: Inkvec.defaultsJSON), reference)
        XCTAssertEqual(try Inkvec.trace(png, options: Inkvec.defaults), reference)
    }

    // MARK: Inputs

    func testAFileTracesAsItsBytes() throws {
        let url = try Repo.contract.appendingPathComponent("tiny.png")
        XCTAssertEqual(try Inkvec.trace(contentsOf: url), try Inkvec.trace(tiny()))
    }

    func testAMissingFileIsAFileError() throws {
        let url = try Repo.contract.appendingPathComponent("no-such-file.png")
        XCTAssertThrowsError(try Inkvec.trace(contentsOf: url)) { error in
            XCTAssertFalse(error is InkvecError, "\(error)")
        }
    }

    func testRawPixelsTraceLikeTheirPNG() throws {
        let rgba = try Repo.data("bindings/contract/tiny.rgba")
        XCTAssertEqual(try Inkvec.traceRGBA(rgba, width: 96, height: 96), try Inkvec.trace(tiny()))
    }

    // MARK: Errors

    func testErrorsMapToTheirKinds() throws {
        let png = try tiny()
        func kind(_ body: () throws -> Traced) -> String? {
            do {
                _ = try body()
                return nil
            } catch let error as InkvecError {
                XCTAssertFalse(error.message.isEmpty)
                XCTAssertEqual(error.localizedDescription, error.message)
                XCTAssertEqual("\(error)", error.message)
                return error.kind
            } catch {
                XCTFail("not an InkvecError: \(error)")
                return nil
            }
        }
        XCTAssertEqual(kind { try Inkvec.trace(Data("not an image".utf8)) }, "invalid_image")
        XCTAssertEqual(kind { try Inkvec.trace(Data()) }, "invalid_image")
        XCTAssertEqual(kind { try Inkvec.traceRGBA(Data(count: 16), width: 3, height: 1) }, "invalid_image")
        XCTAssertEqual(kind { try Inkvec.traceRGBA(Data(count: 16), width: -2, height: -2) }, "invalid_image")
        XCTAssertEqual(kind { try Inkvec.trace(png, optionsJSON: "{not json") }, "invalid_options")
        XCTAssertEqual(kind { try Inkvec.trace(png, optionsJSON: "[]") }, "invalid_options")
    }

    func testAnUnknownOptionIsNamed() throws {
        XCTAssertThrowsError(try Inkvec.trace(tiny(), optionsJSON: #"{"no_such_option_here": 1}"#)) { error in
            guard case .invalidOptions(let message) = error as? InkvecError else {
                return XCTFail("\(error)")
            }
            XCTAssertTrue(message.hasPrefix("invalid options: "), message)
            XCTAssertTrue(message.contains("no_such_option_here"), message)
        }
    }

    // MARK: Threads

    func testConcurrentTracesAgree() throws {
        let png = try tiny()
        let reference = try Inkvec.trace(png).svg
        let lock = NSLock()
        var results: [Result<String, Error>] = []
        DispatchQueue.concurrentPerform(iterations: 6) { _ in
            let outcome = Result { try Inkvec.trace(png).svg }
            lock.lock()
            results.append(outcome)
            lock.unlock()
        }
        XCTAssertEqual(results.count, 6)
        for outcome in results {
            XCTAssertEqual(try outcome.get(), reference)
        }
    }

    // MARK: The C module

    func testTheHeaderCopyIsCurrent() throws {
        func text(_ path: String) throws -> String {
            String(decoding: try Repo.data(path), as: UTF8.self).replacingOccurrences(of: "\r\n", with: "\n")
        }
        XCTAssertEqual(
            try text("packages/swift/Sources/InkvecFFI/inkvec.h"),
            try text("crates/inkvec-ffi/include/inkvec.h"),
            "packages/swift/Sources/InkvecFFI/inkvec.h is stale; run python bindings/codegen/generate.py")
    }
}
