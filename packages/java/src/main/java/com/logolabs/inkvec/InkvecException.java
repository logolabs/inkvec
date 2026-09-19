package com.logolabs.inkvec;

/**
 * Base class of every error Inkvec raises. Always carries the human-readable message the
 * native library produced.
 */
public class InkvecException extends RuntimeException {
    private static final long serialVersionUID = 1L;

    InkvecException(String message) {
        super(message);
    }
}
