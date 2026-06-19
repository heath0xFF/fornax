import Foundation

struct ChatMessage: Identifiable, Codable, Sendable {
    var id: UUID = UUID()
    var role: Role
    var content: String
    var createdAt: Date = Date()

    enum Role: String, Codable, Sendable {
        case user, assistant, system
    }
}
