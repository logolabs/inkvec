<?php

declare(strict_types=1);

namespace LogoLabs\Inkvec;

/**
 * An option the tracer does not know, or a value of the wrong type or out of its range
 * (`INKVEC_ERR_INVALID_OPTIONS`). The message names the option.
 */
final class InvalidOptionsException extends InkvecException
{
}
