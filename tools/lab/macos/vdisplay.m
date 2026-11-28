// A virtual display for the macOS runner's pictures: GitHub's runners have one 1024x768 screen
// at one pixel a point, too small and too coarse for MDM's showcase. This adds a display of
// <width>x<height> points drawn at two pixels a point (HiDPI, as a Retina Mac draws), makes it the
// main display (the menu bar and new windows go there) and mirrors the runner's own screen to it,
// so everything shows on it alone. The display lasts while this program runs: it prints
// "ready <display id>" and waits until it's ended.
//
// CGVirtualDisplay is CoreGraphics' own class (what the system's screen sharing and display apps
// use) but not in its public headers: its interface is declared here. A lab tool only; nothing of
// MDM uses it.
//
//   clang -fobjc-arc -framework Foundation -framework CoreGraphics -o vdisplay vdisplay.m
//   vdisplay <width> <height> &
#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>

@interface CGVirtualDisplayDescriptor : NSObject
@property(retain, nonatomic) dispatch_queue_t queue;
@property(retain, nonatomic) NSString *name;
@property(nonatomic) unsigned int maxPixelsWide;
@property(nonatomic) unsigned int maxPixelsHigh;
@property(nonatomic) CGSize sizeInMillimeters;
@property(nonatomic) unsigned int serialNum;
@property(nonatomic) unsigned int productID;
@property(nonatomic) unsigned int vendorID;
@end

@interface CGVirtualDisplayMode : NSObject
- (instancetype)initWithWidth:(unsigned int)width height:(unsigned int)height refreshRate:(double)refreshRate;
@end

@interface CGVirtualDisplaySettings : NSObject
@property(nonatomic) unsigned int hiDPI;
@property(retain, nonatomic) NSArray *modes;
@end

@interface CGVirtualDisplay : NSObject
@property(readonly, nonatomic) CGDirectDisplayID displayID;
- (instancetype)initWithDescriptor:(CGVirtualDisplayDescriptor *)descriptor;
- (BOOL)applySettings:(CGVirtualDisplaySettings *)settings;
@end

int main(int argc, const char *argv[]) {
  @autoreleasepool {
    unsigned int w = argc > 1 ? (unsigned int)atoi(argv[1]) : 1920;
    unsigned int h = argc > 2 ? (unsigned int)atoi(argv[2]) : 1080;
    CGVirtualDisplayDescriptor *d = [[CGVirtualDisplayDescriptor alloc] init];
    d.queue = dispatch_get_main_queue();
    d.name = @"Showcase";
    d.maxPixelsWide = w * 2;
    d.maxPixelsHigh = h * 2;
    // A 27-inch screen's size: macOS works out the pixels per inch from it.
    d.sizeInMillimeters = CGSizeMake(597, 597.0 * h / w);
    d.serialNum = 1;
    d.productID = 0x1234;
    d.vendorID = 0x3456;
    CGVirtualDisplay *display = [[CGVirtualDisplay alloc] initWithDescriptor:d];
    if (!display) {
      fprintf(stderr, "no virtual display\n");
      return 1;
    }
    CGVirtualDisplaySettings *s = [[CGVirtualDisplaySettings alloc] init];
    s.hiDPI = 1;
    s.modes = @[ [[CGVirtualDisplayMode alloc] initWithWidth:w height:h refreshRate:60] ];
    if (![display applySettings:s]) {
      fprintf(stderr, "settings refused\n");
      return 1;
    }
    CGDirectDisplayID vid = display.displayID;
    // Its HiDPI mode (w x h points, 2w x 2h pixels), once the system lists it.
    for (int i = 0; i < 50; i++) {
      CFArrayRef modes = CGDisplayCopyAllDisplayModes(vid, (__bridge CFDictionaryRef) @{(__bridge NSString *)kCGDisplayShowDuplicateLowResolutionModes : @YES});
      CGDisplayModeRef pick = NULL;
      for (CFIndex m = 0; modes && m < CFArrayGetCount(modes); m++) {
        CGDisplayModeRef mode = (CGDisplayModeRef)CFArrayGetValueAtIndex(modes, m);
        if (CGDisplayModeGetWidth(mode) == w && CGDisplayModeGetPixelWidth(mode) == w * 2) pick = mode;
      }
      if (pick) {
        CGDisplayConfigRef cfg;
        CGBeginDisplayConfiguration(&cfg);
        CGConfigureDisplayWithDisplayMode(cfg, vid, pick, NULL);
        // The main display is the one at 0,0; the runner's own screen shows the same (mirrored).
        CGConfigureDisplayOrigin(cfg, vid, 0, 0);
        CGConfigureDisplayMirrorOfDisplay(cfg, CGMainDisplayID(), vid);
        CGError e = CGCompleteDisplayConfiguration(cfg, kCGConfigureForSession);
        if (modes) CFRelease(modes);
        if (e != kCGErrorSuccess) fprintf(stderr, "configuration: error %d\n", e);
        break;
      }
      if (modes) CFRelease(modes);
      [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.2]];
    }
    CGDisplayModeRef now = CGDisplayCopyDisplayMode(vid);
    printf("ready %u %zux%zu points, %zux%zu pixels\n", vid, CGDisplayModeGetWidth(now), CGDisplayModeGetHeight(now),
           CGDisplayModeGetPixelWidth(now), CGDisplayModeGetPixelHeight(now));
    fflush(stdout);
    CGDisplayModeRelease(now);
    [[NSRunLoop mainRunLoop] run];
  }
  return 0;
}
