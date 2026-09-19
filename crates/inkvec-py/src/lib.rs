//! The native half of the `inkvec` Python package (`inkvec._inkvec`).
//!
//! Deliberately knows nothing about individual options. Python passes its keyword arguments
//! as one JSON object, and `inkvec::Options::from_json` parses and validates them against the
//! same schema the stub file (`python/inkvec/__init__.pyi`) is generated from. Adding an
//! option to Inkvec therefore changes nothing here.
//!
//! Everything is behind the `python` feature, which maturin enables; a plain
//! `cargo build --workspace` compiles this crate empty and needs no Python.

#[cfg(feature = "python")]
mod module {
    use pyo3::create_exception;
    use pyo3::exceptions::PyException;
    use pyo3::prelude::*;

    create_exception!(
        _inkvec,
        InkvecError,
        PyException,
        "Base class of every error Inkvec raises."
    );
    create_exception!(
        _inkvec,
        InvalidImageError,
        InkvecError,
        "The input is not a decodable image, or raw pixels do not match the size given."
    );
    create_exception!(
        _inkvec,
        InvalidOptionsError,
        InkvecError,
        "An option is unknown, of the wrong type, or out of range."
    );
    create_exception!(
        _inkvec,
        InternalError,
        InkvecError,
        "The tracer failed. Not the caller's fault; worth a bug report."
    );

    /// A finished trace: the SVG document and the input's size in pixels.
    #[pyclass(frozen, module = "inkvec")]
    pub(crate) struct Traced {
        /// The SVG document.
        #[pyo3(get)]
        svg: String,
        /// Width of the input image, in pixels.
        #[pyo3(get)]
        width: u32,
        /// Height of the input image, in pixels.
        #[pyo3(get)]
        height: u32,
    }

    #[pymethods]
    impl Traced {
        fn __str__(&self) -> &str {
            &self.svg
        }

        fn __repr__(&self) -> String {
            format!(
                "<inkvec.Traced {}x{}, {} bytes of SVG>",
                self.width,
                self.height,
                self.svg.len()
            )
        }

        /// Rich display in Jupyter and IPython.
        fn _repr_svg_(&self) -> &str {
            &self.svg
        }
    }

    impl From<inkvec::Traced> for Traced {
        fn from(t: inkvec::Traced) -> Self {
            Self {
                svg: t.svg,
                width: t.width,
                height: t.height,
            }
        }
    }

    /// The Python exception for an Inkvec error.
    fn to_py(e: inkvec::Error) -> PyErr {
        let msg = e.message().to_string();
        match e {
            inkvec::Error::InvalidImage(_) => InvalidImageError::new_err(msg),
            inkvec::Error::InvalidOptions(_) => InvalidOptionsError::new_err(msg),
            _ => InternalError::new_err(msg),
        }
    }

    /// Trace encoded image bytes; `options_json` is a JSON object of options.
    #[pyfunction]
    fn _trace(py: Python<'_>, data: &[u8], options_json: &str) -> PyResult<Traced> {
        let opts = inkvec::Options::from_json(options_json).map_err(to_py)?;
        // The trace takes seconds on a large logo; other Python threads keep running.
        py.detach(|| inkvec::trace(data, &opts))
            .map(Traced::from)
            .map_err(to_py)
    }

    /// Trace raw straight-RGBA8 pixels; `options_json` is a JSON object of options.
    #[pyfunction]
    fn _trace_rgba(
        py: Python<'_>,
        data: &[u8],
        width: u32,
        height: u32,
        options_json: &str,
    ) -> PyResult<Traced> {
        let opts = inkvec::Options::from_json(options_json).map_err(to_py)?;
        py.detach(|| inkvec::trace_rgba(data, width, height, &opts))
            .map(Traced::from)
            .map_err(to_py)
    }

    /// The JSON Schema of the options, as text.
    #[pyfunction]
    fn _options_schema_json() -> &'static str {
        inkvec::options_schema_json()
    }

    /// Every option at its default, as a JSON object.
    #[pyfunction]
    fn _default_options_json() -> String {
        inkvec::Options::default().to_json()
    }

    /// The target the native library was compiled for.
    #[pyfunction]
    fn _build_target() -> &'static str {
        inkvec::build_target()
    }

    /// The native module.
    #[pymodule]
    fn _inkvec(m: &Bound<'_, PyModule>) -> PyResult<()> {
        let py = m.py();
        m.add("__version__", inkvec::version())?;
        m.add_class::<Traced>()?;
        m.add_function(wrap_pyfunction!(_trace, m)?)?;
        m.add_function(wrap_pyfunction!(_trace_rgba, m)?)?;
        m.add_function(wrap_pyfunction!(_options_schema_json, m)?)?;
        m.add_function(wrap_pyfunction!(_default_options_json, m)?)?;
        m.add_function(wrap_pyfunction!(_build_target, m)?)?;
        m.add("InkvecError", py.get_type::<InkvecError>())?;
        m.add("InvalidImageError", py.get_type::<InvalidImageError>())?;
        m.add("InvalidOptionsError", py.get_type::<InvalidOptionsError>())?;
        m.add("InternalError", py.get_type::<InternalError>())?;
        Ok(())
    }
}
