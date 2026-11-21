# Tutorial: download overnight

[← Back to docs index](README.md)

Queue big files during the day and let MDM download them at night, when nobody needs the connection. It can turn the computer off when it is done.

## 1. Put the downloads in a queue

For new downloads:

1. Add the download as usual (**Add URL**, or from the browser).
2. In **Download File Info**, click **Download Later**.
3. Under **Add to queue**, pick **Main download queue**. Leave **Start the queue now** unticked. Click **OK**.

For downloads already in the list:

1. Select them (**Ctrl+click** for several).
2. Right-click and choose **Add to queue → Main download queue**.

The **Q** column now shows a queue icon for each of them.

![Download File Info with Download Later ringed](images/help/overnight-later.webp)
*Download Later puts it in a queue instead of starting it.*

## 2. Set the start time

1. Click **Scheduler** on the toolbar (or **Downloads → Scheduler**).
2. Select **Main download queue** on the left. The **Schedule** tab opens.
3. Tick **Start download at** and set the time, for example 1:00:00 AM. Click the hours and use the arrows or type the number.
4. Choose **Daily** and tick the days, or **Once at** and pick a date.
5. To stop in the morning whatever is left, tick **Stop download at** and set a time, for example 7:00:00 AM.

![The Scheduler's Schedule tab: Start download at, Once at or Daily, and Stop download at](images/help/overnight-start.webp)
*1 the start time, 2 once or daily, 3 a stop time if you want one.*

## 3. Choose how many run at once

1. Open the **Files in the queue** tab.
2. Set **Download [N] files at the same time**. Four is the default. Use 1 to download them one by one.
3. The list shows the order they will run in. Use the arrow buttons under it to move a download up or down.

## 4. Decide what happens at the end (optional)

Back on the **Schedule** tab:

- Tick **Turn off computer when done** and choose **Shut down**, **Sleep** or **Hibernate**. MDM shows a 30-second countdown first, so you can still cancel if you're at the computer.
- Or tick **Exit Mister Download Manager when done**.

![The Scheduler's choices for when the queue is done, Turn off computer when done among them](images/help/overnight-done.webp)
*When the queue is done: open a file, quit MDM, or turn the computer off.*

## 5. Apply and leave MDM running

1. Click **Apply**, then **Close**.
2. Leave MDM running. Closing the main window is fine: MDM keeps going in the tray. Don't use **Tasks → Exit**.
3. Make sure the computer doesn't go to sleep before the start time. Once the downloads are running, MDM keeps the computer from going to sleep by itself until they're done; the screen still turns off as usual.

At the start time the queue starts on its own. In the morning, the finished files are under **Finished** in the categories.

## Next steps

- [Everything the Scheduler can do →](queues-and-scheduler.md)
- [Keep some speed for everything else →](guide-keep-speed-for-everything-else.md)
- [If the queue didn't start →](troubleshooting.md#the-scheduler-didnt-start-the-queue)
