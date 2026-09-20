<?php

declare(strict_types=1);

namespace LogoLabs\Inkvec;

/**
 * Anything that went wrong in a trace. The message is the native library's own wording,
 * except where this binding could not reach it at all (an unreadable file, a library that
 * will not load).
 */
class InkvecException extends \RuntimeException
{
}
