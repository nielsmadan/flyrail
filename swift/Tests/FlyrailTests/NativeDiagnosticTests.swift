import Darwin
@testable import Flyrail
import Foundation
import Testing

private struct NativeReferenceError: Decodable {
    let code: String
    let message: String
    let path: String
    let errno: Int32
}

private func nativePythonError(_ path: ResourcePath, operation: String) throws -> TargetError {
    let repository = stateRepositoryRoot()
    let process = Process()
    process.executableURL = try repositoryFile("python/.venv/bin/python")
    process.currentDirectoryURL = repository
    process.arguments = [
        "-B", "-c",
        """
        import json, os, sys
        from pathlib import Path
        sys.path.insert(0, "python/src")
        from flyrail._security import security, ensure_private
        from flyrail._lifecycle import error_detail
        path = Path(sys.argv[1])
        try:
            if sys.argv[2] == "private":
                ensure_private(path)
            elif sys.argv[2] == "syscall":
                os.lstat(path)
            else:
                security(path)
        except OSError as exception:
            error = error_detail(exception)
            print(json.dumps(dict(code=error.code.value, message=error.message,
                                  path=str(error.path), errno=error.errno)))
        """,
        path.displayString, operation,
    ]
    let output = Pipe()
    process.standardOutput = output
    try process.run()
    let data = output.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    #expect(process.terminationStatus == 0)
    let reference = try JSONDecoder().decode(NativeReferenceError.self, from: data)
    return try TargetError(
        code: #require(ErrorCode(rawValue: reference.code)), message: reference.message,
        path: ResourcePath(reference.path), errno: reference.errno
    )
}

private func nativeExactError(_ expected: TargetError, _ body: () throws -> Void) {
    do {
        try body()
        Issue.record("expected \(expected)")
    } catch let actual as TargetError {
        #expect(actual == expected)
    } catch {
        Issue.record("unexpected error: \(error)")
    }
}

@Test(.enabled(if: repositoryCheckout))
func nativeSecurityDiagnosticsMatchCompletePythonErrors() throws {
    _ = try repositoryFile("python/src/flyrail")
    for name in ["plain", "single'quote", "both'\"quotes", "controls\n\r\t\u{1}\u{7F}\u{85}\u{2028}"] {
        let root = try NativeTestRoot()
        defer { try? root.cleanup() }
        let path = try root.child(name)
        try Data("untouched".utf8).write(to: URL(fileURLWithPath: path.displayString))
        let raw = open(path.displayString, O_RDONLY | O_NOFOLLOW)
        let descriptor = try NativeDescriptor(#require(raw >= 0 ? raw : nil))
        #expect(fchmod(raw, 0o644) == 0)
        try nativeExactError(nativePythonError(path, operation: "private")) {
            try NativeSecurity.ensurePrivate(descriptor, path: path)
        }
        #expect(fchmod(raw, 0o600) == 0)
        #expect(fchflags(raw, UInt32(UF_HIDDEN)) == 0)
        defer { _ = fchflags(raw, 0) }
        try nativeExactError(nativePythonError(path, operation: "security")) {
            _ = try NativeObservation.snapshot(path)
        }
        #expect(fchflags(raw, 0) == 0)
        #expect(fsetxattr(raw, "flyrail.diagnostics", nil, 0, 0, 0) == 0)
        try nativeExactError(nativePythonError(path, operation: "security")) {
            _ = try NativeObservation.snapshot(path)
        }
        #expect(fremovexattr(raw, "flyrail.diagnostics", 0) == 0)
        let acl = try #require(acl_from_text(
            "!#acl 1\ngroup:ABCDEFAB-CDEF-ABCD-EFAB-CDEF0000000C:everyone:12:allow:read\n"
        ))
        defer { acl_free(UnsafeMutableRawPointer(acl)) }
        #expect(acl_set_fd_np(raw, acl, ACL_TYPE_EXTENDED) == 0)
        try nativeExactError(nativePythonError(path, operation: "security")) {
            _ = try NativeObservation.snapshot(path)
        }
        let emptyACL = try #require(acl_init(0))
        defer { acl_free(UnsafeMutableRawPointer(emptyACL)) }
        #expect(acl_set_fd_np(raw, emptyACL, ACL_TYPE_EXTENDED) == 0)
        #expect(try NativeObservation.snapshot(path).revision.data == Data("untouched".utf8))
        let oversized = try root.child(name + String(repeating: "x", count: 256))
        try nativeExactError(nativePythonError(oversized, operation: "syscall")) {
            _ = try NativeFilesystem.metadata(oversized)
        }
    }
}
