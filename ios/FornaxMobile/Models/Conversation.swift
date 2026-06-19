import Foundation

struct Conversation: Identifiable, Codable, Sendable {
    var id: UUID = UUID()
    var title: String = "New Chat"
    var modelId: String = ""
    var createdAt: Date = Date()
    var messages: [ChatMessage] = []
}
