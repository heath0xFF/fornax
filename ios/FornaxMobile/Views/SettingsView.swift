import SwiftUI

struct SettingsView: View {
    @AppStorage("endpoint") var endpoint: String = "http://spark:8080"
    @AppStorage("apiKey") var apiKey: String = ""
    @AppStorage("selectedModel") var selectedModel: String = ""
    @AppStorage("appearance") var appearance: String = "system"
    @Environment(\.dismiss) private var dismiss

    @State private var models: [String] = []
    @State private var loadingModels = false
    @State private var modelError: String?

    private let client = LLMClient()

    var body: some View {
        NavigationStack {
            Form {
                Section("Server") {
                    TextField("Endpoint", text: $endpoint)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                        .keyboardType(.URL)
                    SecureField("API Key (optional)", text: $apiKey)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                }

                Section("Appearance") {
                    Picker("Theme", selection: $appearance) {
                        Text("System").tag("system")
                        Text("Light").tag("light")
                        Text("Dark").tag("dark")
                    }
                    .pickerStyle(.segmented)
                }

                Section("Model") {
                    if loadingModels {
                        HStack {
                            ProgressView()
                            Text("Fetching models…").foregroundStyle(.secondary)
                        }
                    } else if models.isEmpty {
                        Button("Load Models") { fetchModels() }
                    } else {
                        Picker("Model", selection: $selectedModel) {
                            ForEach(models, id: \.self) { Text($0).tag($0) }
                        }
                        .pickerStyle(.menu)
                        Button("Refresh") { fetchModels() }
                            .font(.footnote)
                    }
                    if let err = modelError {
                        Text(err).font(.caption).foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .onAppear { if models.isEmpty { fetchModels() } }
        }
    }

    private func fetchModels() {
        loadingModels = true
        modelError = nil
        Task {
            do {
                let fetched = try await client.fetchModels(endpoint: endpoint, apiKey: apiKey)
                await MainActor.run {
                    models = fetched
                    if !fetched.isEmpty && !fetched.contains(selectedModel) {
                        selectedModel = fetched[0]
                    }
                    loadingModels = false
                }
            } catch {
                await MainActor.run {
                    modelError = error.localizedDescription
                    loadingModels = false
                }
            }
        }
    }
}
