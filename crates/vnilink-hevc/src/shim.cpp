// Native ownership stays here. No exception or borrowed native pointer crosses C ABI.
#include <kvazaar.h>
#include <libde265/de265.h>
#include <cstdint>
#include <cstring>
#include <mutex>
#include <new>
#include <vector>
#include <string>
#include <memory>
static std::mutex kvz_lock; // Kvazaar's strategy dispatch is process-global.
struct Encoder {
  const kvz_api* api = kvz_api_get(8);
  kvz_config* config = nullptr;
  kvz_encoder* owner = nullptr;
  int width, height;
  bool first = true;
  std::vector<uint8_t> packet;
  Encoder(int w, int h) : width(w), height(h) {}
  ~Encoder() { if(owner) api->encoder_close(owner); if(config) api->config_destroy(config); }
  bool option(const char* key, int value) { return api->config_parse(config,key,std::to_string(value).c_str()); }
};
extern "C" void* vnl_hevc_encoder_open(int w,int h,int fps,int bitrate,int threads) noexcept {
  try {
    std::lock_guard<std::mutex> lock(kvz_lock);
    auto storage=std::make_unique<Encoder>(w,h);
    Encoder* e=storage.get();
    if(!e->api || !(e->config=e->api->config_alloc())) { return nullptr; }
    bool valid=e->api->config_init(e->config) && e->option("enable-logging",0)
      && e->api->config_parse(e->config,"preset","ultrafast")
      && e->option("width",w) && e->option("height",h) && e->option("input-fps",fps)
      && e->option("bitrate",bitrate) && e->option("gop",0) && e->option("owf",0)
      && e->option("threads",threads) && e->option("period",120) && e->option("vps-period",1);
    if(valid) e->owner=e->api->encoder_open(e->config);
    if(!e->owner) { return nullptr; }
    return storage.release();
  } catch(...) { return nullptr; }
}
extern "C" void vnl_hevc_encoder_close(void* owner) noexcept {
  std::lock_guard<std::mutex> lock(kvz_lock); delete static_cast<Encoder*>(owner);
}
extern "C" int vnl_hevc_encode(void* owner,const uint8_t* pixels,size_t length,int force,
                               const uint8_t** output,size_t* output_length,int* key) noexcept {
  try {
    std::lock_guard<std::mutex> lock(kvz_lock);
    auto* e=static_cast<Encoder*>(owner);
    size_t luma=size_t(e->width)*e->height;
    if(length!=luma*3/2 || !e->owner) return 0;
    if(force && !e->first) {
      e->api->encoder_close(e->owner); e->owner=e->api->encoder_open(e->config); e->first=true;
      if(!e->owner) return 0;
    }
    auto* picture=e->api->picture_alloc(e->width,e->height);
    if(!picture) return 0;
    const uint8_t* planes[]={pixels,pixels+luma,pixels+luma+luma/4};
    for(int p=0;p<3;p++) {
      int w=e->width/(p?2:1),h=e->height/(p?2:1),stride=picture->stride/(p?2:1);
      for(int y=0;y<h;y++) std::memcpy(picture->data[p]+size_t(y)*stride,planes[p]+size_t(y)*w,w);
    }
    kvz_data_chunk* chunks=nullptr; uint32_t bytes=0; kvz_frame_info info{};
    int ok=e->api->encoder_encode(e->owner,picture,&chunks,&bytes,nullptr,nullptr,&info);
    e->api->picture_free(picture);
    // owf=0 and gop=0 guarantee one synchronous output for every supplied frame.
    if(!ok || !chunks || !bytes || bytes>1024*1024) { e->api->chunk_free(chunks); return 0; }
    e->packet.clear();
    try {
      e->packet.reserve(bytes);
      for(auto* c=chunks;c;c=c->next) {
        if(c->len>KVZ_DATA_CHUNK_SIZE || c->len>bytes-e->packet.size()) {
          e->api->chunk_free(chunks); return 0;
        }
        e->packet.insert(e->packet.end(),c->data,c->data+c->len);
      }
    } catch(...) { e->api->chunk_free(chunks); throw; }
    e->api->chunk_free(chunks);
    if(e->packet.size()!=bytes) return 0;
    *key=info.nal_unit_type>=16 && info.nal_unit_type<=21;
    *output=e->packet.data(); *output_length=e->packet.size(); e->first=false;
    return 1;
  } catch(...) { return 0; }
}
static int allocate_picture(de265_decoder_context* context,de265_image_spec* spec,de265_image* image,void* data) {
  // Reject hostile SPS dimensions before the native allocator reserves pixels.
  if(spec->width<=0 || spec->height<=0 || spec->width>4096 || spec->height>4096
     || spec->format!=de265_image_format_YUV420P8 || spec->alignment<=0 || spec->alignment>256) return 0;
  return de265_get_default_image_allocation_functions()->get_buffer(context,spec,image,data);
}
static void release_picture(de265_decoder_context* context,de265_image* image,void* data) {
  de265_get_default_image_allocation_functions()->release_buffer(context,image,data);
}
static de265_image_allocation allocation={allocate_picture,release_picture};
struct Decoder {
  de265_decoder_context* owner=de265_new_decoder();
  std::vector<uint8_t> picture;
  ~Decoder() { if(owner) de265_free_decoder(owner); }
};
extern "C" void* vnl_hevc_decoder_open() noexcept {
  try { auto* d=new Decoder; if(!d->owner) { delete d; return nullptr; }
    de265_set_image_allocation_functions(d->owner,&allocation,nullptr);
    de265_set_parameter_bool(d->owner,DE265_DECODER_PARAM_SUPPRESS_FAULTY_PICTURES,1);
    return d;
  } catch(...) { return nullptr; }
}
extern "C" void vnl_hevc_decoder_close(void* owner) noexcept { delete static_cast<Decoder*>(owner); }
extern "C" int vnl_hevc_decode(void* owner,const uint8_t* packet,size_t length,
                               const uint8_t** output,size_t* output_length,int* width,int* height) noexcept {
  try {
    auto* d=static_cast<Decoder*>(owner);
    struct ResetOnFailure {de265_decoder_context* owner;bool success=false;~ResetOnFailure(){if(!success)de265_reset(owner);}} reset{d->owner};
    if(!length || length>1024*1024 || !de265_isOK(de265_push_data(d->owner,packet,int(length),0,nullptr))) return 0;
    de265_push_end_of_frame(d->owner);
    for(int n=0;n<512;n++) {
      if(de265_peek_next_picture(d->owner)) break;
      int more=0; auto result=de265_decode(d->owner,&more);
      if(result==DE265_ERROR_WAITING_FOR_INPUT_DATA || !de265_isOK(result) || !more) break;
    }
    const auto* image=de265_peek_next_picture(d->owner);
    if(!image) return 0;
    int w=de265_get_image_width(image,0),h=de265_get_image_height(image,0);
    if(w<=0 || h<=0 || w>4096 || h>4096 || w%2 || h%2 || de265_get_chroma_format(image)!=de265_chroma_420) return 0;
    d->picture.resize(size_t(w)*h*3/2); size_t offset=0;
    for(int p=0;p<3;p++) {
      int pw=w/(p?2:1),ph=h/(p?2:1),stride=0;
      const auto* plane=de265_get_image_plane(image,p,&stride);
      if(!plane || stride<pw || de265_get_bits_per_pixel(image,p)!=8
         || de265_get_image_width(image,p)!=pw || de265_get_image_height(image,p)!=ph) return 0;
      for(int y=0;y<ph;y++) std::memcpy(d->picture.data()+offset+size_t(y)*pw,plane+size_t(y)*stride,pw);
      offset+=size_t(pw)*ph;
    }
    de265_release_next_picture(d->owner);
    if(de265_peek_next_picture(d->owner)) return 0;
    *output=d->picture.data(); *output_length=d->picture.size(); *width=w; *height=h;reset.success=true;
    return 1;
  } catch(...) { return 0; }
}
