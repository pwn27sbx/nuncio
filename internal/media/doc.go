// Package media downloads WhatsApp attachments on explicit request, verifies
// them against the SHA-256 the message carries and caches the plaintext in a
// private directory (dir 0700, files 0600).
//
// The network fetch and the decryption with the media key are delegated to
// whatsmeow's Client.Download, which *whatsmeow.Client satisfies as a
// Downloader. This package owns what comes after: the cache file name, a
// second, independent SHA-256 check, the atomic private write and serving a
// second request from the cache. Nothing here runs in the background:
// constructing a Cache does no I/O and nothing is fetched until Fetch is
// called.
package media
