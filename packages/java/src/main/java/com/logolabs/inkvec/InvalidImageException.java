package com.logolabs.inkvec;

/** The input is not a decodable image, or raw pixels do not match the size given. */
public final class InvalidImageException extends InkvecException {
    private static final long serialVersionUID = 1L;

    InvalidImageException(String message) {
        super(message);
    }
}
