import Darwin
@testable import Flyrail
import Foundation
import Testing

private func storeFailure(_ body: () throws -> Void) -> StoreError? {
    do {
        try body()
        return nil
    } catch let error as StoreError {
        return error
    } catch {
        return nil
    }
}

private func mode(_ path: String) -> mode_t? {
    var metadata = stat()
    return lstat(path, &metadata) == 0 ? metadata.st_mode & 0o777 : nil
}

@Test
func aHeldStoreLockMakesAnotherHandleBusyAfterTheBoundedWait() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let competing = try DirectoryStore(ResourcePath(root.stateURL.path))
    let name = try StateKey.resourceLock(root.child("document"))
    let held = try root.store.lock(name, timeout: .zero)
    defer { held.release() }
    let wait = Duration.milliseconds(100)
    let started = ContinuousClock.now
    #expect(storeFailure { _ = try competing.lock(name, timeout: wait) } == .busy(name: name))
    #expect(started.duration(to: .now) >= wait)
    let other = try competing.lock(StateKey.resourceLock(root.child("other")), timeout: .zero)
    other.release()
    held.release()
    let freed = try competing.lock(name, timeout: .zero)
    freed.release()
}

@Test
func lockFilesArePrivateAndAGroupReadableLocksDirectoryIsRefused() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let name = try StateKey.resourceLock(root.child("document"))
    let held = try root.store.lock(name, timeout: .zero)
    defer { held.release() }
    let locks = root.stateURL.appending(path: "locks")
    let file = locks.appending(path: "resource~" + name.dropFirst("resource/".count))
    #expect(mode(file.path) == 0o600)
    #expect(chmod(locks.path, 0o750) == 0)
    let refusal = storeFailure { _ = try root.store.lock("other", timeout: .zero) }
    #expect(try refusal == .io(
        message: "[Errno 13] state store must be private to its owner: \(pythonStringRepr(locks.path))",
        path: ResourcePath(locks.path), errno: EACCES,
    ))
}

@Test
func syncReportsBusyWhenAnotherHandleHoldsTheResourceLock() throws {
    let root = try NativeTestRoot()
    defer { try? root.cleanup() }
    let path = try root.child("document")
    let ref = try NativeAuthority.reference(path, kind: .document)
    let name = try StateKey.resourceLock(ref.destination)
    let competing = try DirectoryStore(ResourcePath(root.stateURL.path))
    let held = try competing.lock(name, timeout: .zero)
    defer { held.release() }
    let wait = Duration.milliseconds(50)
    let started = ContinuousClock.now
    let result = try sync(bundle: transactionBundle(), rendered: transactionRendered(path), target: root.target(),
                          lockTimeout: wait)
    #expect(started.duration(to: .now) >= wait)
    #expect(result.error == TargetError(code: .busy, message: "state store lock is busy: \(name)",
                                        path: root.store.location))
    #expect(try NativeFilesystem.metadata(path) == nil)
}
