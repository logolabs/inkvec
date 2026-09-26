"""OpenAPI 3.1 spec for the HTTP service (`crates/inkvec-server`), generated from
`bindings/options.schema.json`.

    python bindings/codegen/generate.py           # regenerates this among everything else
    python bindings/codegen/generate.py --check    # fails if bindings/openapi.json is stale

JSON Schema draft 2020-12 -- what `schemars` writes and OpenAPI 3.1 uses -- are the same
dialect, so `components.schemas.Options` is the committed `options.schema.json` embedded
verbatim, not restated. The server serves this file's output byte for byte at `/openapi.json`
(`crates/inkvec-server/src/lib.rs` embeds it with `include_str!`); the per-option query
parameters on `POST /trace` are generated from the same `Option` list every other generator in
this folder reads.
"""

from __future__ import annotations

import json
import tomllib
from pathlib import Path

from schema import ROOT, SCHEMA_PATH, Option

OUT = ROOT / "bindings" / "openapi.json"

ERROR_CODES = ["invalid_image", "invalid_options", "internal", "too_large", "busy"]


def _version() -> str:
    data = tomllib.loads((ROOT / "Cargo.toml").read_text("utf-8"))
    return data["workspace"]["package"]["version"]


def _error_response(description: str) -> dict:
    return {
        "description": description,
        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}},
    }


def _query_param(o: Option) -> dict:
    prop: dict = {"type": o.kind}
    if o.minimum is not None:
        prop["minimum"] = o.minimum
    if o.maximum is not None:
        prop["maximum"] = o.maximum
    if o.exclusive_minimum is not None:
        prop["exclusiveMinimum"] = o.exclusive_minimum
    if o.exclusive_maximum is not None:
        prop["exclusiveMaximum"] = o.exclusive_maximum
    if o.choices is not None:
        prop["enum"] = list(o.choices)
    return {
        "name": o.name,
        "in": "query",
        "required": False,
        "description": o.description,
        "schema": prop,
    }


def render(options: list[Option]) -> dict[Path, str]:
    options_schema = json.loads(SCHEMA_PATH.read_text("utf-8"))

    spec = {
        "openapi": "3.1.0",
        "info": {
            "title": "Inkvec HTTP service",
            "version": _version(),
            "description": (
                "Raster-to-SVG tracing over HTTP. Every option is inkvec::Options end to end: "
                "this document, and the query parameters on POST /trace below, are generated "
                "from the same JSON Schema the Rust facade writes "
                "(bindings/options.schema.json) -- nothing about an option is restated here."
            ),
        },
        "paths": {
            "/trace": {
                "post": {
                    "summary": "Trace a raster image to SVG",
                    "description": (
                        "The body is raw image bytes (any image/* or application/octet-stream "
                        "Content-Type), or multipart/form-data with an `image` part and an "
                        "optional `options` JSON part. Options are read from, lowest to "
                        "highest priority: query parameters matching the schema below, the "
                        "`options` query parameter (URL-encoded JSON), the X-Inkvec-Options "
                        "header (JSON), and a multipart `options` part -- a later source "
                        "overrides a key an earlier one set."
                    ),
                    "parameters": [
                        {
                            "name": "options",
                            "in": "query",
                            "required": False,
                            "description": "The full options object, as URL-encoded JSON.",
                            "schema": {"type": "string"},
                        },
                        {
                            "name": "X-Inkvec-Options",
                            "in": "header",
                            "required": False,
                            "description": "The full options object, as JSON.",
                            "schema": {"type": "string"},
                        },
                        *[_query_param(o) for o in options],
                    ],
                    "requestBody": {
                        "required": True,
                        "content": {
                            "image/*": {"schema": {"type": "string", "contentMediaType": "image/*", "contentEncoding": "binary"}},
                            "application/octet-stream": {"schema": {"type": "string", "contentEncoding": "binary"}},
                            "multipart/form-data": {
                                "schema": {
                                    "type": "object",
                                    "required": ["image"],
                                    "properties": {
                                        "image": {"type": "string", "contentEncoding": "binary"},
                                        "options": {"$ref": "#/components/schemas/Options"},
                                    },
                                }
                            },
                        },
                    },
                    "responses": {
                        "200": {
                            "description": (
                                "The SVG document. X-Inkvec-Width and X-Inkvec-Height carry "
                                "the input's pixel size."
                            ),
                            "headers": {
                                "X-Inkvec-Width": {"schema": {"type": "integer"}},
                                "X-Inkvec-Height": {"schema": {"type": "integer"}},
                            },
                            "content": {"image/svg+xml": {"schema": {"type": "string"}}},
                        },
                        "400": _error_response("invalid_image: not a decodable image"),
                        "413": _error_response("too_large: the body exceeded the server's limit"),
                        "422": _error_response("invalid_options: unknown option, wrong type, or out of range"),
                        "500": _error_response("internal: the pipeline failed, panicked, or the request timed out"),
                        "503": _error_response("busy: at the server's concurrency limit"),
                    },
                }
            },
            "/options/schema": {
                "get": {
                    "summary": "The options JSON Schema (bindings/options.schema.json)",
                    "responses": {
                        "200": {
                            "description": "OK",
                            "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Options"}}},
                        }
                    },
                }
            },
            "/options/defaults": {
                "get": {
                    "summary": "Every option at its default",
                    "responses": {
                        "200": {
                            "description": "OK",
                            "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Options"}}},
                        }
                    },
                }
            },
            "/healthz": {
                "get": {
                    "summary": "Liveness check",
                    "responses": {"200": {"description": "OK"}},
                }
            },
            "/version": {
                "get": {
                    "summary": "Crate version and build target",
                    "responses": {
                        "200": {
                            "description": "OK",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "version": {"type": "string"},
                                            "build_target": {"type": "string"},
                                        },
                                    }
                                }
                            },
                        }
                    },
                }
            },
            "/openapi.json": {
                "get": {
                    "summary": "This document",
                    "responses": {"200": {"description": "OK"}},
                }
            },
        },
        "components": {
            "schemas": {
                "Options": options_schema,
                "Error": {
                    "type": "object",
                    "required": ["error"],
                    "properties": {
                        "error": {
                            "type": "object",
                            "required": ["code", "message"],
                            "properties": {
                                "code": {"type": "string", "enum": ERROR_CODES},
                                "message": {"type": "string"},
                            },
                        }
                    },
                },
            }
        },
    }
    text = json.dumps(spec, indent=2) + "\n"
    return {OUT: text}
