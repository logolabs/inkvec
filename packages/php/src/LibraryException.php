<?php

declare(strict_types=1);

namespace LogoLabs\Inkvec;

/**
 * The native library could not be used at all: ext-FFI missing or disabled, no
 * `libinkvec_ffi` found on this machine, or one whose C ABI version this binding does not
 * speak. Nothing was traced.
 */
final class LibraryException extends InkvecException
{
}
