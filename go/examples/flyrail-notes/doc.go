// Command flyrail-notes counts the words in a notes file and installs its
// companion agent skill from a bundle directory with Flyrail.
//
// Usage:
//
//	flyrail-notes --version
//	flyrail-notes words FILE
//	flyrail-notes ai status|install --source DIR --target DIR [--target DIR ...] --state DIR
//	flyrail-notes ai update --source DIR --target DIR [--target DIR ...] --state DIR [--replace-modified]
//	flyrail-notes ai uninstall --bundle-id ID --target DIR [--target DIR ...] --state DIR [--replace-modified]
//
// The ai commands print a JSON report. Exit status 0 means success (or, for
// status, that every target is current), 1 that a target differs or an
// operation did not complete, and 2 a usage or source error. Uninstall needs
// only the bundle ID and the state directory, so it works after the bundle
// directory is gone.
package main
