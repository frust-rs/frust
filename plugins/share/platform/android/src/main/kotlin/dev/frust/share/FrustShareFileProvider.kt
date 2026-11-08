package dev.frust.share

import androidx.core.content.FileProvider

/**
 * The share module's own [FileProvider]. A subclass so its manifest entry
 * never collides with an app-declared `androidx.core.content.FileProvider`.
 * Authority and exposed paths are declared in this module's manifest and
 * `res/xml/frust_share_paths.xml`.
 */
class FrustShareFileProvider : FileProvider()
