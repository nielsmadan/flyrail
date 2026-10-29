import FlyrailInteropSupport
import Foundation

let status = runInterop(
    arguments: Array(CommandLine.arguments.dropFirst()),
    standardInput: .standardInput,
    standardOutput: .standardOutput,
    standardError: .standardError
)
exit(status)
