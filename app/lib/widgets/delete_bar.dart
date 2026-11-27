import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';

/// Deletes [id] after [HomeNode.deleteWait], with a bar saying so and an
/// Undo meanwhile (ADR 0019). The bar goes once the delete is sent: at the
/// end of the wait, or sooner if the owner leaves the app.
void deleteWithUndo(
  ScaffoldMessengerState messenger,
  HomeNode node,
  String id, {
  required String text,
}) {
  // Only the bar on screen can be closed, so note when it is.
  var showing = false;
  final bar = messenger.showSnackBar(
    SnackBar(
      content: Text(text),
      duration: HomeNode.deleteWait,
      // A bar with an action would otherwise stay until it's tapped.
      persist: false,
      onVisible: () => showing = true,
      action: SnackBarAction(
        label: 'Undo',
        onPressed: () => node.undoDelete(id),
      ),
    ),
  );
  bar.closed.whenComplete(() => showing = false);
  node.deleteLater(
    id,
    onSending: () {
      if (showing) bar.close();
    },
    onFailed: (e) => messenger.showSnackBar(
      SnackBar(content: Text("Couldn't delete it: ${describeError(e)}")),
    ),
  );
}
