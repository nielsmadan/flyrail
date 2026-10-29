import Foundation
import Testing

let repositoryCheckout: Bool = {
    var directory: ObjCBool = false
    return FileManager.default.fileExists(atPath: stateRepositoryRoot().appending(path: "spec").path,
                                          isDirectory: &directory) && directory.boolValue
}()

func repositoryFile(_ path: String, sourceLocation: SourceLocation = #_sourceLocation) throws -> URL {
    let url = stateRepositoryRoot().appending(path: path)
    try #require(FileManager.default.fileExists(atPath: url.path),
                 "repository checkout is missing \(path)", sourceLocation: sourceLocation)
    return url
}
