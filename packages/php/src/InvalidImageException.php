<?php

declare(strict_types=1);

namespace LogoLabs\Inkvec;

/**
 * The bytes are not an image Inkvec can decode, or the raw pixels do not match the width
 * and height given (`INKVEC_ERR_INVALID_IMAGE`).
 */
final class InvalidImageException extends InkvecException
{
}
