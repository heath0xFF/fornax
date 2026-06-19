import SwiftUI
import Observation

@MainActor
@Observable
final class ChatViewModel {
    var conversation: Conversation
    var inputText: String = ""
    var isStreaming = false
    var lastMetrics: StreamMetrics?
    var errorMessage: String?

    var onAdd: ((Conversation) -> Void)?
    var onUpdate: ((Conversation) -> Void)?

    private let client = LLMClient()
    private var streamTask: Task<Void, Never>?

    init(conversation: Conversation) {
        self.conversation = conversation
    }

    func send(endpoint: String, apiKey: String, model: String) {
        let text = inputText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !isStreaming else { return }
        inputText = ""
        errorMessage = nil
        lastMetrics = nil

        let isFirstMessage = conversation.messages.isEmpty

        conversation.messages.append(ChatMessage(role: .user, content: text))
        conversation.modelId = model

        // Optimistic title from first message — replaced by generated title after response
        if isFirstMessage {
            conversation.title = String(text.prefix(40))
        }

        var assistantMsg = ChatMessage(role: .assistant, content: "")
        conversation.messages.append(assistantMsg)
        let assistantIdx = conversation.messages.count - 1

        isStreaming = true

        if isFirstMessage {
            onAdd?(conversation)  // register with store on first send
        } else {
            onUpdate?(conversation)
        }

        streamTask = Task {
            do {
                let stream = client.streamChat(
                    endpoint: endpoint,
                    apiKey: apiKey,
                    model: model,
                    messages: Array(conversation.messages.dropLast())
                )
                for try await update in stream {
                    switch update {
                    case .token(let t):
                        assistantMsg.content += t
                        conversation.messages[assistantIdx] = assistantMsg
                    case .metrics(let m):
                        lastMetrics = m
                    case .error(let e):
                        errorMessage = e
                    }
                }
            } catch {
                errorMessage = error.localizedDescription
                if conversation.messages[assistantIdx].content.isEmpty {
                    conversation.messages.remove(at: assistantIdx)
                }
            }
            isStreaming = false
            onUpdate?(conversation)

            // Generate a real title after the first response
            if isFirstMessage {
                await generateTitle(endpoint: endpoint, apiKey: apiKey, model: model)
            }
        }
    }

    func cancel() {
        streamTask?.cancel()
        streamTask = nil
        isStreaming = false
    }

    private func generateTitle(endpoint: String, apiKey: String, model: String) async {
        var messages = conversation.messages
        messages.append(ChatMessage(role: .user, content: "Write a short 4-6 word title for this conversation. Reply with ONLY the title — no quotes, no punctuation, no explanation."))
        guard let raw = try? await client.fetchTitle(endpoint: endpoint, apiKey: apiKey, model: model, messages: messages) else { return }
        let title = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "\"'"))
        guard !title.isEmpty else { return }
        conversation.title = title
        onUpdate?(conversation)
    }
}

struct ChatView: View {
    @State private var vm: ChatViewModel
    @AppStorage("endpoint") private var endpoint: String = "http://spark:8080"
    @AppStorage("apiKey") private var apiKey: String = ""
    @AppStorage("selectedModel") private var selectedModel: String = ""
    @State private var showSettings = false
    @State private var showModelPicker = false

    let onAdd: ((Conversation) -> Void)?
    let onUpdate: (Conversation) -> Void

    init(conversation: Conversation, onAdd: ((Conversation) -> Void)? = nil, onUpdate: @escaping (Conversation) -> Void) {
        _vm = State(wrappedValue: ChatViewModel(conversation: conversation))
        self.onAdd = onAdd
        self.onUpdate = onUpdate
    }

    private var effectiveModel: String {
        vm.conversation.modelId.isEmpty ? selectedModel : vm.conversation.modelId
    }

    var body: some View {
        VStack(spacing: 0) {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 12) {
                        ForEach(vm.conversation.messages) { msg in
                            MessageBubble(message: msg)
                                .id(msg.id)
                        }
                    }
                    .padding(.horizontal)
                    .padding(.top, 8)
                    .padding(.bottom, 8)
                }
                .safeAreaInset(edge: .bottom) { Color.clear.frame(height: 20) }
                .onChange(of: vm.conversation.messages.count) {
                    if let last = vm.conversation.messages.last {
                        withAnimation { proxy.scrollTo(last.id, anchor: .bottom) }
                    }
                }
                .onChange(of: vm.conversation.messages.last?.content) {
                    if let last = vm.conversation.messages.last {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }

            if let metrics = vm.lastMetrics {
                MetricsBar(metrics: metrics)
            }

            if let err = vm.errorMessage {
                Text(err)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .padding(.horizontal)
            }

            Divider()
            inputBar
        }
        .toolbar {
            ToolbarItem(placement: .principal) {
                VStack(spacing: 1) {
                    Text(vm.conversation.title)
                        .font(.headline)
                    Button { showModelPicker = true } label: {
                        HStack(spacing: 3) {
                            Text(effectiveModel.isEmpty ? "No model selected" : effectiveModel)
                                .font(.caption)
                            Image(systemName: "chevron.down")
                                .font(.system(size: 8, weight: .semibold))
                        }
                        .foregroundStyle(.secondary)
                    }
                    .buttonStyle(.plain)
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button { showSettings = true } label: {
                    Image(systemName: "gear")
                }
            }
        }
        .sheet(isPresented: $showSettings) { SettingsView() }
        .sheet(isPresented: $showModelPicker) {
            ModelPickerSheet(selectedModel: $selectedModel)
        }
        .onAppear {
            vm.onAdd = onAdd
            vm.onUpdate = onUpdate
        }
    }

    private var inputBar: some View {
        HStack(spacing: 8) {
            TextField("Message", text: $vm.inputText, axis: .vertical)
                .lineLimit(1...6)
                .padding(10)
                .background(Color(.secondarySystemBackground))
                .clipShape(RoundedRectangle(cornerRadius: 12))
                .onSubmit { sendOrCancel() }

            Button(action: sendOrCancel) {
                Image(systemName: vm.isStreaming ? "stop.circle.fill" : "arrow.up.circle.fill")
                    .font(.system(size: 32))
                    .foregroundStyle(vm.isStreaming ? .red : .blue)
            }
            .disabled(!vm.isStreaming && vm.inputText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
        .padding(.horizontal)
        .padding(.vertical, 8)
    }

    private func sendOrCancel() {
        if vm.isStreaming {
            vm.cancel()
        } else {
            vm.send(endpoint: endpoint, apiKey: apiKey, model: selectedModel)
        }
    }
}

private struct ModelPickerSheet: View {
    @Binding var selectedModel: String
    @AppStorage("endpoint") private var endpoint: String = "http://spark:8080"
    @AppStorage("apiKey") private var apiKey: String = ""
    @State private var models: [String] = []
    @State private var loading = false
    @State private var fetchError: String?
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Group {
                if loading {
                    ProgressView("Fetching models…")
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else if models.isEmpty {
                    ContentUnavailableView {
                        Label(fetchError != nil ? "Couldn't load models" : "No models found", systemImage: "cpu")
                    } description: {
                        if let err = fetchError { Text(err).font(.caption) }
                    } actions: {
                        Button("Retry") { fetch() }
                    }
                } else {
                    List(models, id: \.self) { model in
                        Button {
                            selectedModel = model
                            dismiss()
                        } label: {
                            HStack {
                                Text(model).foregroundStyle(.primary)
                                Spacer()
                                if model == selectedModel {
                                    Image(systemName: "checkmark")
                                        .foregroundStyle(.blue)
                                        .fontWeight(.semibold)
                                }
                            }
                        }
                    }
                }
            }
            .navigationTitle("Model")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
            .onAppear { fetch() }
        }
    }

    private func fetch() {
        loading = true
        fetchError = nil
        Task {
            do {
                models = try await LLMClient().fetchModels(endpoint: endpoint, apiKey: apiKey)
            } catch {
                fetchError = error.localizedDescription
            }
            loading = false
        }
    }
}

struct MessageBubble: View {
    let message: ChatMessage

    var isUser: Bool { message.role == .user }

    var body: some View {
        if isUser {
            HStack {
                Spacer(minLength: 48)
                Text(message.content)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(Color.blue)
                    .foregroundStyle(.white)
                    .clipShape(RoundedRectangle(cornerRadius: 16))
                    .textSelection(.enabled)
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
        } else if message.content.isEmpty {
            HStack {
                TypingIndicator()
                    .padding(.horizontal, 16)
                    .padding(.vertical, 14)
                    .background(Color(.secondarySystemBackground))
                    .clipShape(RoundedRectangle(cornerRadius: 16))
                Spacer(minLength: 48)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            MarkdownText(content: message.content)
                .padding(.horizontal, 4)
        }
    }
}

struct MetricsBar: View {
    let metrics: StreamMetrics

    var body: some View {
        HStack(spacing: 16) {
            if let ttft = metrics.ttft {
                Label(String(format: "TTFT %.0fms", ttft * 1000), systemImage: "timer")
            }
            if let tps = metrics.tokensPerSecond {
                Label(String(format: "%.1f tok/s", tps), systemImage: "bolt")
            }
            if let n = metrics.completionTokens {
                Label("\(n) tokens", systemImage: "character.cursor.ibeam")
            }
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .padding(.horizontal)
        .padding(.vertical, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(.secondarySystemBackground))
    }
}
