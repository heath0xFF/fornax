import Foundation

struct StreamMetrics: Sendable {
    var ttft: TimeInterval?
    var tokensPerSecond: Double?
    var completionTokens: Int?
}

enum StreamUpdate: Sendable {
    case token(String)
    case metrics(StreamMetrics)
    case error(String)
}

struct LLMClient: Sendable {

    func fetchModels(endpoint: String, apiKey: String) async throws -> [String] {
        guard let url = URL(string: endpoint.trimmingCharacters(in: .init(charactersIn: "/")) + "/v1/models") else {
            throw URLError(.badURL)
        }
        var req = URLRequest(url: url)
        if !apiKey.isEmpty { req.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization") }
        let (data, _) = try await URLSession.shared.data(for: req)
        struct ModelsResponse: Decodable { struct Model: Decodable { let id: String }; let data: [Model] }
        let decoded = try JSONDecoder().decode(ModelsResponse.self, from: data)
        return decoded.data.map(\.id).sorted()
    }

    func fetchTitle(endpoint: String, apiKey: String, model: String, messages: [ChatMessage]) async throws -> String {
        guard let url = URL(string: endpoint.trimmingCharacters(in: .init(charactersIn: "/")) + "/v1/chat/completions") else {
            throw URLError(.badURL)
        }
        struct Msg: Encodable { let role: String; let content: String }
        struct Body: Encodable { let model: String; let messages: [Msg]; let stream: Bool; let max_tokens: Int }

        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if !apiKey.isEmpty { req.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization") }
        req.httpBody = try JSONEncoder().encode(Body(
            model: model,
            messages: messages.map { Msg(role: $0.role.rawValue, content: $0.content) },
            stream: false,
            max_tokens: 12
        ))

        let (data, _) = try await URLSession.shared.data(for: req)
        struct Choice: Decodable { struct Msg: Decodable { let content: String }; let message: Msg }
        struct Response: Decodable { let choices: [Choice] }
        return try JSONDecoder().decode(Response.self, from: data).choices.first?.message.content ?? ""
    }

    func streamChat(
        endpoint: String,
        apiKey: String,
        model: String,
        messages: [ChatMessage]
    ) -> AsyncThrowingStream<StreamUpdate, Error> {
        AsyncThrowingStream { continuation in
            Task.detached {
                do {
                    guard let url = URL(string: endpoint.trimmingCharacters(in: .init(charactersIn: "/")) + "/v1/chat/completions") else {
                        throw URLError(.badURL)
                    }

                    struct Msg: Encodable { let role: String; let content: String }
                    struct Body: Encodable {
                        let model: String
                        let messages: [Msg]
                        let stream: Bool = true
                        let stream_options: [String: Bool] = ["include_usage": true]
                    }

                    var req = URLRequest(url: url)
                    req.httpMethod = "POST"
                    req.setValue("application/json", forHTTPHeaderField: "Content-Type")
                    if !apiKey.isEmpty { req.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization") }
                    req.httpBody = try JSONEncoder().encode(Body(
                        model: model,
                        messages: messages.map { Msg(role: $0.role.rawValue, content: $0.content) }
                    ))

                    let sendTime = Date()
                    var firstTokenTime: Date?
                    var lastChunkTime: Date?
                    var completionTokens: Int?

                    let session = URLSession(configuration: .ephemeral)
                    let (bytes, _) = try await session.bytes(for: req)

                    for try await line in bytes.lines {
                        guard line.hasPrefix("data: ") else { continue }
                        let payload = String(line.dropFirst(6))
                        if payload == "[DONE]" { break }
                        guard let data = payload.data(using: .utf8) else { continue }

                        struct Delta: Decodable { let content: String? }
                        struct Choice: Decodable { let delta: Delta }
                        struct Usage: Decodable { let completion_tokens: Int }
                        struct Chunk: Decodable { let choices: [Choice]; let usage: Usage? }

                        guard let chunk = try? JSONDecoder().decode(Chunk.self, from: data) else { continue }

                        if let usage = chunk.usage {
                            completionTokens = usage.completion_tokens
                        }

                        if let content = chunk.choices.first?.delta.content, !content.isEmpty {
                            let now = Date()
                            if firstTokenTime == nil { firstTokenTime = now }
                            lastChunkTime = now
                            continuation.yield(.token(content))
                        }
                    }

                    var metrics = StreamMetrics()
                    if let ft = firstTokenTime {
                        metrics.ttft = ft.timeIntervalSince(sendTime)
                        if let lt = lastChunkTime, let tokens = completionTokens, lt > ft {
                            metrics.tokensPerSecond = Double(tokens) / lt.timeIntervalSince(ft)
                            metrics.completionTokens = tokens
                        }
                    }
                    continuation.yield(.metrics(metrics))
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
        }
    }
}
