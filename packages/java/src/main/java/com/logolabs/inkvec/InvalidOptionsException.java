package com.logolabs.inkvec;

/**
 * An option is unknown, of the wrong type, or out of range (including a malformed options
 * JSON string, when you pass options as raw JSON).
 */
public final class InvalidOptionsException extends InkvecException {
    private static final long serialVersionUID = 1L;

    InvalidOptionsException(String message) {
        super(message);
    }
}
