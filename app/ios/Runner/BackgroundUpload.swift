import BackgroundTasks
import UIKit

/// Keeps the app running while a post is sent to the home node.
///
/// The sending itself happens in the app's Rust code (osp-client). This
/// only asks iOS not to suspend the app once it leaves the screen. On iOS
/// 26 and later that's a continued processing task, which shows the post's
/// progress in a Live Activity, where the owner can also stop it. Before
/// iOS 26 the app gets about 30 seconds. If iOS suspends the app anyway,
/// the post carries on when the app is next opened.
///
/// Everything here runs on the main thread: the method channel calls it
/// there, and the task's handlers are sent there.
final class BackgroundUpload {
  // A post is being sent.
  private var wanted = false

  // The continued processing task while it runs. Typed as its base class,
  // which older iOS versions have too.
  private var task: BGTask?

  // A short background task: the app's time once it leaves the screen
  // until the continued processing task starts, or before iOS 26.
  private var shortTask = UIBackgroundTaskIdentifier.invalid

  // What the Live Activity shows.
  private var title = "Sending your post"
  private var text = ""
  private var percent: Int?

  /// Starts keeping the app running. Only works while the app is on
  /// screen, after the owner tapped something.
  func start() {
    finishTask(success: false)
    wanted = true
    title = "Sending your post"
    text = ""
    percent = nil
    beginShortTask()
    if #available(iOS 26, *) {
      submitTask()
    }
  }

  /// Updates the Live Activity. `percent` nil leaves the progress as it was.
  func show(title: String, text: String, percent: Int?) {
    self.title = title
    self.text = text
    self.percent = percent
    update()
  }

  /// Stops keeping the app running. `sent` says whether the post was
  /// published, for the Live Activity.
  func stop(sent: Bool) {
    wanted = false
    finishTask(success: sent)
    endShortTask()
  }

  @available(iOS 26, *)
  private func submitTask() {
    // Each task needs its own name, under the one in Info.plist, and its
    // own handler: iOS refuses one handler for all of them.
    let id = "\(Bundle.main.bundleIdentifier ?? "").upload.\(UUID().uuidString)"
    let registered = BGTaskScheduler.shared.register(forTaskWithIdentifier: id, using: .main) {
      [weak self] task in
      guard let task = task as? BGContinuedProcessingTask else { return }
      self?.run(task)
    }
    guard registered else { return }
    let request = BGContinuedProcessingTaskRequest(identifier: id, title: title, subtitle: text)
    // Start now or not at all. The short task covers the first moments
    // either way.
    request.strategy = .fail
    do {
      try BGTaskScheduler.shared.submit(request)
    } catch {
      NSLog("background upload: \(error)")
    }
  }

  @available(iOS 26, *)
  private func run(_ task: BGContinuedProcessingTask) {
    guard wanted, self.task == nil else {
      // The post ended before the task started, or another one runs.
      task.setTaskCompleted(success: true)
      return
    }
    self.task = task
    // The owner stopped it, or iOS needs the room. The app may be
    // suspended now; the post waits until it's next opened.
    task.expirationHandler = { [weak self, weak task] in
      DispatchQueue.main.async {
        guard let self, let task, self.task === task else { return }
        self.finishTask(success: false)
      }
    }
    task.progress.totalUnitCount = 100
    update()
  }

  private func update() {
    guard #available(iOS 26, *) else { return }
    guard let task = task as? BGContinuedProcessingTask else { return }
    task.updateTitle(title, subtitle: text)
    if let percent {
      // All sent isn't done yet: the node still prepares the post.
      task.progress.completedUnitCount = Int64(min(percent, 99))
    }
  }

  private func finishTask(success: Bool) {
    guard let task else { return }
    self.task = nil
    if #available(iOS 26, *), success, let task = task as? BGContinuedProcessingTask {
      task.progress.completedUnitCount = task.progress.totalUnitCount
    }
    task.setTaskCompleted(success: success)
  }

  private func beginShortTask() {
    guard shortTask == .invalid else { return }
    shortTask = UIApplication.shared.beginBackgroundTask(withName: "Sending a post") {
      [weak self] in
      // Time's up: iOS ends the app if it's still held.
      self?.endShortTask()
    }
  }

  private func endShortTask() {
    guard shortTask != .invalid else { return }
    UIApplication.shared.endBackgroundTask(shortTask)
    shortTask = .invalid
  }
}
