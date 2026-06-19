import SwiftUI

struct ConversationListView: View {
    @Environment(ConversationStore.self) private var store
    @AppStorage("selectedModel") private var selectedModel: String = ""
    @State private var selectedId: UUID?
    @State private var pendingConversation: Conversation?
    @State private var showSettings = false

    private var activeConversation: Conversation? {
        guard let id = selectedId else { return nil }
        if let pending = pendingConversation, pending.id == id { return pending }
        return store.conversations.first(where: { $0.id == id })
    }

    var body: some View {
        NavigationSplitView {
            List(selection: $selectedId) {
                ForEach(store.conversations) { conv in
                    NavigationLink(value: conv.id) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(conv.title)
                                .lineLimit(1)
                            Text(conv.modelId.isEmpty ? "no model" : conv.modelId)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                        }
                    }
                }
                .onDelete(perform: store.delete)
            }
            .navigationTitle("Fornax")
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button { showSettings = true } label: {
                        Image(systemName: "gear")
                    }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        let c = Conversation(modelId: selectedModel)
                        pendingConversation = c
                        selectedId = c.id
                    } label: {
                        Image(systemName: "square.and.pencil")
                    }
                }
            }
            .sheet(isPresented: $showSettings) { SettingsView() }
            .onChange(of: selectedId) { _, newId in
                // Discard pending conversation if the user navigates away without sending
                if let pending = pendingConversation, newId != pending.id {
                    pendingConversation = nil
                }
            }
        } detail: {
            if let conv = activeConversation {
                let isPending = pendingConversation?.id == conv.id
                ChatView(
                    conversation: conv,
                    onAdd: isPending ? { saved in
                        store.add(saved)
                        pendingConversation = nil
                    } : nil,
                    onUpdate: { updated in
                        store.update(updated)
                    }
                )
                .id(conv.id)
            } else {
                ContentUnavailableView(
                    "No Chat Selected",
                    systemImage: "bubble.left.and.bubble.right",
                    description: Text(selectedModel.isEmpty ? "Set a model in Settings" : "Model: \(selectedModel)")
                )
            }
        }
    }
}
