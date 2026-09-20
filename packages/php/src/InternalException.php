<?php

declare(strict_types=1);

namespace LogoLabs\Inkvec;

/**
 * The tracer itself failed or panicked (`INKVEC_ERR_INTERNAL`). Not the caller's fault:
 * worth a bug report at https://github.com/logolabs/inkvec/issues.
 */
final class InternalException extends InkvecException
{
}
