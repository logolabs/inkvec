package com.logolabs.inkvec;

/** The tracer failed or panicked. Not the caller's fault; worth a bug report. */
public final class InternalException extends InkvecException {
    private static final long serialVersionUID = 1L;

    InternalException(String message) {
        super(message);
    }
}
