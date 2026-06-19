import Foundation
import Observation

@MainActor
@Observable
final class ConversationStore {
    var conversations: [Conversation] = []

    private let fileURL: URL = {
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        return docs.appendingPathComponent("conversations.json")
    }()

    init() { load() }

    func load() {
        guard let data = try? Data(contentsOf: fileURL),
              let decoded = try? JSONDecoder().decode([Conversation].self, from: data)
        else { return }
        conversations = decoded
    }

    func save() {
        guard let data = try? JSONEncoder().encode(conversations) else { return }
        try? data.write(to: fileURL, options: .atomic)
    }

    func add(_ conversation: Conversation) {
        conversations.insert(conversation, at: 0)
        save()
    }

    func update(_ conversation: Conversation) {
        if let idx = conversations.firstIndex(where: { $0.id == conversation.id }) {
            conversations[idx] = conversation
        }
        save()
    }

    func delete(at offsets: IndexSet) {
        conversations.remove(atOffsets: offsets)
        save()
    }
}
