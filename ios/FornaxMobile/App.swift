import SwiftUI

@main
struct FornaxMobileApp: App {
    @State private var store = ConversationStore()
    @AppStorage("appearance") private var appearance: String = "system"

    private var colorScheme: ColorScheme? {
        switch appearance {
        case "light": return .light
        case "dark":  return .dark
        default:      return nil
        }
    }

    var body: some Scene {
        WindowGroup {
            ConversationListView()
                .environment(store)
                .preferredColorScheme(colorScheme)
        }
    }
}
